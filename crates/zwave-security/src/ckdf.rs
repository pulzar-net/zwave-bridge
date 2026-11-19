//! S2 key derivation (CKDF), per the Security 2 specification.
//!
//! All derivations are AES-128-CMAC over fixed constants, so the outputs are
//! the same on every implementation; the controller and the joining node
//! derive the same temporary keys from the ECDH secret and both public keys.

use crate::crypto::cmac;

const CONST_PRK: [u8; 16] = [0x33; 16];
const CONST_TE: [u8; 15] = [0x88; 15];
const CONST_NK: [u8; 15] = [0x55; 15];
const CONST_NONCE: [u8; 16] = [0x26; 16];
const CONST_EI: [u8; 15] = [0x88; 15];

/// Keys derived from a permanent network key (or, during bootstrapping, from the ECDH secret).
#[derive(Clone, PartialEq, Eq, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct DerivedKeys {
    /// The AES-CCM key for encapsulation.
    pub key_ccm: [u8; 16],
    /// The CTR_DRBG personalization string for SPAN nonces.
    pub personalization: [u8; 32],
    /// The key that turns the MPAN inner state into multicast nonces (zero for temporary keys).
    pub key_mpan: [u8; 16],
}

impl std::fmt::Debug for DerivedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DerivedKeys(..)")
    }
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// CKDF-TempExtract: the pseudo random key from the ECDH shared secret.
///
/// `pub_a` is the including node's (controller's) public key and `pub_b` the
/// joining node's.
pub fn temp_prk(shared_secret: &[u8; 32], pub_a: &[u8; 32], pub_b: &[u8; 32]) -> [u8; 16] {
    cmac(&CONST_PRK, &cat(&[shared_secret, pub_a, pub_b]))
}

/// CKDF-TempExpand: the temporary CCM key and personalization string.
pub fn temp_keys(prk: &[u8; 16]) -> DerivedKeys {
    let t1 = cmac(prk, &cat(&[&CONST_TE, &[0x01]]));
    let t2 = cmac(prk, &cat(&[&t1, &CONST_TE, &[0x02]]));
    let t3 = cmac(prk, &cat(&[&t2, &CONST_TE, &[0x03]]));
    let mut personalization = [0u8; 32];
    personalization[..16].copy_from_slice(&t2);
    personalization[16..].copy_from_slice(&t3);
    DerivedKeys {
        key_ccm: t1,
        personalization,
        key_mpan: [0; 16],
    }
}

/// CKDF-NetworkKeyExpand: CCM key, personalization string and MPAN key from a network key.
pub fn network_keys(pnk: &[u8; 16]) -> DerivedKeys {
    let t1 = cmac(pnk, &cat(&[&CONST_NK, &[0x01]]));
    let t2 = cmac(pnk, &cat(&[&t1, &CONST_NK, &[0x02]]));
    let t3 = cmac(pnk, &cat(&[&t2, &CONST_NK, &[0x03]]));
    let t4 = cmac(pnk, &cat(&[&t3, &CONST_NK, &[0x04]]));
    let mut personalization = [0u8; 32];
    personalization[..16].copy_from_slice(&t2);
    personalization[16..].copy_from_slice(&t3);
    DerivedKeys {
        key_ccm: t1,
        personalization,
        key_mpan: t4,
    }
}

/// CKDF-MEI-Extract: the nonce PRK from the sender's and receiver's entropy inputs.
pub fn nonce_prk(sender_ei: &[u8; 16], receiver_ei: &[u8; 16]) -> [u8; 16] {
    cmac(&CONST_NONCE, &cat(&[sender_ei, receiver_ei]))
}

/// CKDF-MEI-Expand: the 32-byte mixed entropy input that seeds a SPAN's CTR_DRBG.
pub fn mei(nonce_prk: &[u8; 16]) -> [u8; 32] {
    let t1 = cmac(nonce_prk, &cat(&[&CONST_EI, &[0x00], &CONST_EI, &[0x01]]));
    let t2 = cmac(nonce_prk, &cat(&[&t1, &CONST_EI, &[0x02]]));
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&t1);
    out[16..].copy_from_slice(&t2);
    out
}
