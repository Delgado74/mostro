pub mod backend;
pub mod config;

pub use backend::{ArkadeBackend, PendingBond, PendingVhtlc, VhtlcStatus};

#[cfg(test)]
mod vhtlc_spike {
    #[test]
    fn ark_core_vhtlc_constructs_and_encodes_address() {
        use ark_rs::core::vhtlc::{VhtlcOptions, VhtlcScript};
        use bitcoin::hashes::{ripemd160, Hash};
        use bitcoin::secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
        use bitcoin::{Network, Sequence};

        let secp = Secp256k1::new();
        let sk_sender = SecretKey::from_slice(&[1u8; 32]).unwrap();
        let kp_sender = Keypair::from_secret_key(&secp, &sk_sender);
        let sender = XOnlyPublicKey::from_keypair(&kp_sender).0;

        let sk_receiver = SecretKey::from_slice(&[2u8; 32]).unwrap();
        let kp_receiver = Keypair::from_secret_key(&secp, &sk_receiver);
        let receiver = XOnlyPublicKey::from_keypair(&kp_receiver).0;

        let sk_server = SecretKey::from_slice(&[3u8; 32]).unwrap();
        let kp_server = Keypair::from_secret_key(&secp, &sk_server);
        let server = XOnlyPublicKey::from_keypair(&kp_server).0;

        let preimage_hash = ripemd160::Hash::from_byte_array([0xab; 20]);
        let options = VhtlcOptions {
            sender,
            receiver,
            server,
            preimage_hash,
            refund_locktime: 500,
            unilateral_claim_delay: Sequence::from_height(144),
            unilateral_refund_delay: Sequence::from_height(144),
            unilateral_refund_without_receiver_delay: Sequence::from_height(144),
        };
        assert!(options.validate().is_ok());

        let script = VhtlcScript::new(options, Network::Signet).unwrap();
        let spk = script.script_pubkey();
        assert_eq!(spk.len(), 34); // P2TR
        let addr_str = script.address().encode();
        assert!(!addr_str.is_empty());
    }
}
