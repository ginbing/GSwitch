// Public upstream vectors from iotaledger/crypto.rs at
// 8c13125541e762206e2dc57b8bfde89c7f6ce8e3 (Apache-2.0).
use crypto::{
    keys::slip10::{Seed, Segment},
    signatures::secp256k1_ecdsa::{PublicKey, RecoverableSignature, SecretKey},
};

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn secp256k1_preserves_public_bytes_signatures_and_recovery() {
    let key = SecretKey::try_from_bytes(
        bytes("ea68a753b20281db2cd5084a215fdda70c7e3cf3d167575fce9b02eb1f36fe9c")
            .as_slice()
            .try_into()
            .unwrap(),
    )
    .unwrap();
    let public = PublicKey::try_from_slice(&bytes(
        "02af81cf426b23df0f01c2f6acd86a7fa24cf48c1637c320e06f40dc315b597f6d",
    ))
    .unwrap();
    assert_eq!(key.public_key(), public);
    assert_eq!(
        PublicKey::try_from_bytes(&public.to_bytes()).unwrap(),
        public
    );
    assert!(SecretKey::try_from_bytes(&[0; 32]).is_err());
    assert!(PublicKey::try_from_slice(&[0; 33]).is_err());
    let message = b"GSwitch synthetic upstream compatibility vector";
    let signature = key.try_sign_sha256(message).unwrap();
    let signature = RecoverableSignature::try_from_bytes(&signature.to_bytes()).unwrap();
    assert_eq!(signature.recover_sha256(message), Some(public));
    assert!(public.verify_sha256(signature.as_ref(), message));
    assert!(!public.verify_sha256(signature.as_ref(), b"changed"));
    let prehash = [17; 32];
    let signature = key.try_sign_prehash(&prehash).unwrap();
    assert_eq!(signature.recover_prehash(&prehash), Some(public));
    assert!(public.verify_prehash(signature.as_ref(), &prehash));
    let signature = key.try_sign_keccak256(message).unwrap();
    assert_eq!(signature.recover_keccak256(message), Some(public));
    assert!(public.verify_keccak256(signature.as_ref(), message));
    let generated = SecretKey::generate();
    let signature = generated.try_sign_sha256(message).unwrap();
    assert!(generated
        .public_key()
        .verify_sha256(signature.as_ref(), message));
}

struct TestChain {
    chain: Vec<u32>,
    chain_code: &'static str,
    private_key: &'static str,
}
struct TestVector {
    seed: &'static str,
    master_chain_code: &'static str,
    master_private_key: &'static str,
    chains: Vec<TestChain>,
}

#[test]
fn slip10_preserves_all_upstream_secp256k1_vectors_and_public_derivation() {
    let vectors = include!("fixtures/slip10-secp256k1.rs");
    for vector in vectors {
        let seed = Seed::from_bytes(&bytes(vector.seed));
        let master = seed.to_master_key::<SecretKey>();
        assert_eq!(
            master.chain_code().as_slice(),
            bytes(vector.master_chain_code)
        );
        assert_eq!(
            master.secret_key().to_bytes().as_slice(),
            bytes(vector.master_private_key)
        );
        for chain in vector.chains {
            let child = seed.derive::<SecretKey, _>(chain.chain.iter().copied());
            assert_eq!(child.chain_code().as_slice(), bytes(chain.chain_code));
            assert_eq!(
                child.secret_key().to_bytes().as_slice(),
                bytes(chain.private_key)
            );
            if let Some((last, head)) = chain.chain.split_last() {
                if !last.is_hardened() {
                    let parent = seed.derive::<SecretKey, _>(head.iter().copied());
                    let public = parent
                        .to_extended_public_key()
                        .child_key(slip10_nonhardened(*last));
                    assert_eq!(public.public_key(), child.secret_key().public_key());
                    assert_eq!(public.chain_code(), child.chain_code());
                }
            }
        }
    }
}

fn slip10_nonhardened(segment: u32) -> crypto::keys::slip10::NonHardened {
    segment.try_into().unwrap()
}
