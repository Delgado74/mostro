use serde::{Deserialize, Serialize};

/// Arkade VHTLC escrow backend configuration.
///
/// Read from the `[arkade]` section of `settings.toml`. Absent section ≡
/// disabled (the daemon falls back to Lightning mode).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ArkadeConfig {
    /// Master switch. When false, the Arkade backend is inactive.
    #[serde(default)]
    pub enabled: bool,

    /// URL of the `arkd` daemon (e.g. `http://localhost:8080`).
    /// Required when `enabled = true`; validated at startup.
    #[serde(default)]
    pub arkd_url: String,

    /// Hex-encoded private key of the server (daemon) for co-signing VHTLCs.
    /// Required when `enabled = true`; validated at startup.
    #[serde(default)]
    pub server_key: String,

    /// Hex-encoded public key of the server (daemon).
    /// Derived from `server_key` at startup if not provided.
    #[serde(default)]
    pub server_pubkey: String,

    /// Arkade address of the fee wallet where trade fees are sent.
    /// Format: `ark1<bech32>` or equivalent.
    /// Required when `enabled = true`; validated at startup.
    #[serde(default)]
    pub escrow_fee_wallet: String,

    /// Fee percentage charged to each peer (buyer + seller).
    /// 0.0025 = 0.25% per peer = 0.5% total.
    #[serde(default = "default_fee_pct")]
    pub fee_pct: f64,

    /// Refund locktime in blocks. Inherits from `hold_invoice_cltv_delta`
    /// in `[lightning]` section when not set explicitly. This is the
    /// `refundLocktime` parameter for VHTLC contracts.
    #[serde(default)]
    pub refund_locktime_blocks: Option<u32>,

    /// Unilateral claim delay for VHTLC (CSV relative locktime).
    /// Maps to `VhtlcOptions.unilateral_claim_delay`.
    #[serde(default)]
    pub unilateral_claim_delay: Option<u32>,

    /// Unilateral refund delay for VHTLC (CSV relative locktime).
    /// Maps to `VhtlcOptions.unilateral_refund_delay`.
    #[serde(default)]
    pub unilateral_refund_delay: Option<u32>,

    /// Unilateral refund without receiver delay for VHTLC (CSV relative locktime).
    /// Maps to `VhtlcOptions.unilateral_refund_without_receiver_delay`.
    #[serde(default)]
    pub unilateral_refund_without_receiver_delay: Option<u32>,
}

fn default_fee_pct() -> f64 {
    0.0025
}

impl Default for ArkadeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            arkd_url: String::new(),
            server_key: String::new(),
            server_pubkey: String::new(),
            escrow_fee_wallet: String::new(),
            fee_pct: default_fee_pct(),
            refund_locktime_blocks: None,
            unilateral_claim_delay: None,
            unilateral_refund_delay: None,
            unilateral_refund_without_receiver_delay: None,
        }
    }
}

impl From<&ArkadeConfig> for ArkadeConfig {
    fn from(cfg: &ArkadeConfig) -> Self {
        cfg.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let cfg = ArkadeConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.arkd_url.is_empty());
        assert_eq!(cfg.fee_pct, 0.0025);
        assert!(cfg.refund_locktime_blocks.is_none());
    }

    #[test]
    fn clone_from_reference_preserves_all_fields() {
        let cfg = ArkadeConfig {
            enabled: true,
            arkd_url: "http://localhost:8080".to_string(),
            server_key: "abc".to_string(),
            server_pubkey: "def".to_string(),
            escrow_fee_wallet: "ark1xyz".to_string(),
            fee_pct: 0.005,
            refund_locktime_blocks: Some(432),
            unilateral_claim_delay: None,
            unilateral_refund_delay: None,
            unilateral_refund_without_receiver_delay: None,
        };
        let clone = ArkadeConfig::from(&cfg);
        assert_eq!(cfg.enabled, clone.enabled);
        assert_eq!(cfg.arkd_url, clone.arkd_url);
        assert_eq!(cfg.server_key, clone.server_key);
        assert_eq!(cfg.server_pubkey, clone.server_pubkey);
        assert_eq!(cfg.escrow_fee_wallet, clone.escrow_fee_wallet);
        assert_eq!(cfg.fee_pct, clone.fee_pct);
        assert_eq!(cfg.refund_locktime_blocks, clone.refund_locktime_blocks);
    }
}
