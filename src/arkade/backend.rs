use async_trait::async_trait;
use mostro_core::prelude::*;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::app::bond::slash::SettleLightning;
use crate::arkade::config::ArkadeConfig;
use crate::escrow::EscrowBackend;

/// Pending VHTLC escrow record. Tracked in-memory while the escrow is
/// active; persisted to DB for durability.
#[derive(Debug, Clone)]
pub struct PendingVhtlc {
    /// Order ID this VHTLC belongs to.
    pub order_id: String,
    /// Hex-encoded preimage hash (payment hash of the VHTLC).
    pub preimage_hash: String,
    /// Hex-encoded preimage (only known after settlement).
    pub preimage: Option<String>,
    /// Refund locktime in blocks.
    pub refund_locktime: u32,
    /// Server pubkey (co-signer).
    pub server_pubkey: String,
    /// Receiver pubkey (buyer for sell orders, seller for buy orders).
    pub receiver_pubkey: String,
    /// Sender pubkey (seller for sell orders, buyer for buy orders).
    pub sender_pubkey: String,
    /// Amount in sats.
    pub amount_sats: i64,
    /// Current status.
    pub status: VhtlcStatus,
}

/// Status of a VHTLC escrow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VhtlcStatus {
    /// VHTLC created, waiting for sender to fund it.
    Pending,
    /// VHTLC funded, waiting for server confirmation.
    Funded,
    /// Server confirmed the VHTLC is live on-chain.
    Confirmed,
    /// Claimed by receiver (trade completed).
    Claimed,
    /// Refunded to sender (trade canceled or expired).
    Refunded,
    /// Slashed by server (dispute or timeout).
    Slashed,
}

/// Pending anti-abuse bond VHTLC. Separate from the trade escrow.
#[derive(Debug, Clone)]
pub struct PendingBond {
    /// Order ID this bond belongs to.
    pub order_id: String,
    /// Hex-encoded preimage hash.
    pub preimage_hash: String,
    /// Hex-encoded preimage (only known after settlement).
    pub preimage: Option<String>,
    /// Refund locktime in blocks.
    pub refund_locktime: u32,
    /// Server pubkey (co-signer).
    pub server_pubkey: String,
    /// Bond holder pubkey (maker or taker).
    pub holder_pubkey: String,
    /// Bond amount in sats.
    pub amount_sats: i64,
    /// Current status.
    pub status: VhtlcStatus,
}

/// Arkade VHTLC escrow backend (stub implementation).
///
/// This is a **validation stub** that tracks VHTLC state in memory and
/// logs operations. It does NOT connect to `arkd` — real integration
/// arrives in a later phase. The stub lets us validate the escrow flow
/// logic before wiring up the actual Ark RPC.
///
/// Implements the `EscrowBackend` trait to maintain compatibility with
/// the existing handler infrastructure.
pub struct ArkadeBackend {
    /// Configuration.
    pub config: ArkadeConfig,
    /// Pending VHTLC escrows keyed by preimage hash.
    pub escrows: Arc<RwLock<HashMap<String, PendingVhtlc>>>,
    /// Pending bonds keyed by preimage hash.
    pub bonds: Arc<RwLock<HashMap<String, PendingBond>>>,
}

impl ArkadeBackend {
    pub fn new(config: ArkadeConfig) -> Self {
        Self {
            config,
            escrows: Arc::new(RwLock::new(HashMap::new())),
            bonds: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the refund locktime for VHTLC contracts.
    ///
    /// Uses `refund_locktime_blocks` from config if set, otherwise
    /// falls back to the `hold_invoice_cltv_delta` from Lightning config
    /// (default 144 blocks ≈ 24h).
    pub fn refund_locktime(&self) -> u32 {
        if let Some(blocks) = self.config.refund_locktime_blocks {
            return blocks;
        }
        // Inherit from [lightning].hold_invoice_cltv_delta when the global
        // config is initialized (production); fall back to a 144-block
        // default (~24h) otherwise so unit tests never panic.
        crate::config::MOSTRO_CONFIG
            .get()
            .map(|s| s.lightning.hold_invoice_cltv_delta)
            .filter(|v| *v > 0)
            .unwrap_or(144)
    }

    /// Calculate the fee for a trade amount.
    ///
    /// Returns `(seller_fee, buyer_fee)` in sats. Each peer pays
    /// `fee_pct` of the trade amount.
    pub fn calculate_fees(&self, trade_amount_sats: i64) -> (i64, i64) {
        let fee = (trade_amount_sats as f64 * self.config.fee_pct) as i64;
        (fee, fee)
    }
}

fn arkade_not_implemented(op: &str) -> MostroError {
    MostroInternalErr(ServiceError::HoldInvoiceError(format!(
        "arkade escrow backend: {op} not implemented (validation stub)"
    )))
}

#[async_trait]
impl EscrowBackend for ArkadeBackend {
    /// Create a VHTLC escrow.
    ///
    /// In the real implementation, this would:
    /// 1. Generate a preimage and compute its hash
    /// 2. Call `arkd` to create the VHTLC with the hash
    /// 3. Return the VHTLC identifier (as "bolt11" equivalent)
    ///
    /// For now, this is a stub that generates the hash and stores the
    /// pending escrow in memory.
    async fn create_hold_invoice(
        &mut self,
        description: &str,
        amount: i64,
    ) -> Result<(String, Vec<u8>, Vec<u8>), MostroError> {
        // Generate a random preimage (32 bytes)
        let preimage: [u8; 32] = rand::random();
        let preimage_hex = hex::encode(preimage);

        // Compute SHA256 hash of the preimage
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(preimage);
        let hash = hasher.finalize();
        let hash_hex = hex::encode(hash);

        tracing::info!(
            "ArkadeBackend::create_hold_invoice: order={}, amount={} sats, hash={}",
            description,
            amount,
            hash_hex
        );

        // Store the pending escrow
        let escrow = PendingVhtlc {
            order_id: description.to_string(),
            preimage_hash: hash_hex.clone(),
            preimage: Some(preimage_hex),
            refund_locktime: self.refund_locktime(),
            server_pubkey: self.config.server_pubkey.clone(),
            receiver_pubkey: String::new(), // Set later when order is matched
            sender_pubkey: String::new(),   // Set later when order is matched
            amount_sats: amount,
            status: VhtlcStatus::Pending,
        };

        self.escrows
            .write()
            .await
            .insert(hash_hex.clone(), escrow);

        // Return (vhtlc_id, preimage, hash) — the "bolt11" equivalent
        // is the VHTLC identifier that clients use to track the escrow
        Ok((hash_hex, preimage.to_vec(), hash.to_vec()))
    }

    /// Settle a VHTLC escrow with its preimage.
    ///
    /// In the real implementation, this would:
    /// 1. Look up the VHTLC by preimage hash
    /// 2. Call `arkd` to sign and broadcast the `claim` transaction
    /// 3. The vtxo goes to the receiver
    ///
    /// For now, this is a stub that marks the escrow as claimed.
    async fn settle_hold_invoice(&mut self, preimage: &str) -> Result<(), MostroError> {
        tracing::info!(
            "ArkadeBackend::settle_hold_invoice: preimage={}",
            &preimage[..8.min(preimage.len())]
        );

        // Find the escrow by preimage
        let mut escrows = self.escrows.write().await;
        let escrow = escrows.values_mut().find(|e| {
            e.preimage
                .as_ref()
                .map(|p| p == preimage)
                .unwrap_or(false)
        });

        match escrow {
            Some(e) => {
                e.status = VhtlcStatus::Claimed;
                tracing::info!(
                    "ArkadeBackend: VHTLC for order {} claimed successfully",
                    e.order_id
                );
                Ok(())
            }
            None => Err(arkade_not_implemented(
                "settle_hold_invoice: escrow not found",
            )),
        }
    }

    /// Cancel a VHTLC escrow.
    ///
    /// In the real implementation, this would:
    /// 1. Look up the VHTLC by hash
    /// 2. Call `arkd` to sign and broadcast the `refund` transaction
    /// 3. The vtxo goes back to the sender
    ///
    /// For now, this is a stub that marks the escrow as refunded.
    async fn cancel_hold_invoice(&mut self, hash: &str) -> Result<(), MostroError> {
        tracing::info!(
            "ArkadeBackend::cancel_hold_invoice: hash={}",
            &hash[..8.min(hash.len())]
        );

        let mut escrows = self.escrows.write().await;
        if let Some(escrow) = escrows.get_mut(hash) {
            escrow.status = VhtlcStatus::Refunded;
            tracing::info!(
                "ArkadeBackend: VHTLC for order {} refunded",
                escrow.order_id
            );
            Ok(())
        } else {
            Err(arkade_not_implemented(
                "cancel_hold_invoice: escrow not found",
            ))
        }
    }
}

/// Bond slash path needs a `SettleLightning` receiver on the escrow
/// backend. Arkade mode and anti-abuse bonds are mutually exclusive at
/// config validation, so this is never invoked by the (gated) bond block —
/// it only lets the shared waiting-state tick type-check against the
/// generic trait bound.
impl SettleLightning for ArkadeBackend {
    fn settle_hold_invoice<'a>(
        &'a mut self,
        preimage: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), MostroError>> + Send + 'a>> {
        Box::pin(async move { EscrowBackend::settle_hold_invoice(self, preimage).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ArkadeConfig {
        ArkadeConfig {
            enabled: true,
            arkd_url: "http://localhost:8080".to_string(),
            server_key: "aabbccdd".to_string(),
            server_pubkey: "eeff0011".to_string(),
            escrow_fee_wallet: "ark1test".to_string(),
            fee_pct: 0.0025,
            refund_locktime_blocks: Some(144),
            unilateral_claim_delay: None,
            unilateral_refund_delay: None,
            unilateral_refund_without_receiver_delay: None,
        }
    }

    #[tokio::test]
    async fn arkade_backend_stub_returns_typed_error_without_panicking() {
        let config = test_config();
        let mut backend = ArkadeBackend::new(config);

        // Test settle with unknown preimage
        let settle = crate::escrow::EscrowBackend::settle_hold_invoice(&mut backend, "unknown_preimage").await;
        assert!(settle.is_err());

        // Test cancel with unknown hash
        let cancel = backend.cancel_hold_invoice("unknown_hash").await;
        assert!(cancel.is_err());
    }

    #[tokio::test]
    async fn arkade_backend_create_and_settle() {
        let config = test_config();
        let mut backend = ArkadeBackend::new(config);

        // Create a hold invoice
        let (hash_hex, preimage, hash) = backend
            .create_hold_invoice("test-order-123", 100_000)
            .await
            .unwrap();

        assert_eq!(hash_hex.len(), 64); // SHA256 hex = 64 chars
        assert_eq!(preimage.len(), 32);
        assert_eq!(hash.len(), 32);

        // Settle with the preimage
        let preimage_hex = hex::encode(&preimage);
        let settle = crate::escrow::EscrowBackend::settle_hold_invoice(&mut backend, &preimage_hex).await;
        assert!(settle.is_ok());

        // Verify status
        let escrows = backend.escrows.read().await;
        let escrow = escrows.get(&hash_hex).unwrap();
        assert_eq!(escrow.status, VhtlcStatus::Claimed);
    }

    #[test]
    fn fee_calculation() {
        let config = test_config();
        let backend = ArkadeBackend::new(config);

        let (seller_fee, buyer_fee) = backend.calculate_fees(100_000);
        assert_eq!(seller_fee, 250); // 0.25% of 100,000
        assert_eq!(buyer_fee, 250);

        let (seller_fee, buyer_fee) = backend.calculate_fees(1_000);
        assert_eq!(seller_fee, 2); // 0.25% of 1,000 = 2.5, truncated
        assert_eq!(buyer_fee, 2);
    }

    #[test]
    fn refund_locktime_default() {
        // Without an explicit refund_locktime_blocks, the backend inherits
        // the node's hold_invoice_cltv_delta. With no global config
        // installed (isolation), the fallback default is 144 blocks.
        let mut config = test_config();
        config.refund_locktime_blocks = None;

        let backend = ArkadeBackend::new(config);
        assert_eq!(backend.refund_locktime(), 144);
    }

    #[test]
    fn refund_locktime_explicit() {
        let mut config = test_config();
        config.refund_locktime_blocks = Some(432);

        let backend = ArkadeBackend::new(config);
        assert_eq!(backend.refund_locktime(), 432);
    }
}
