//! Thin wrappers over the RustCrypto primitives in the shapes Z-Wave uses.
//!
//! Every function here is a direct call into a maintained crate: AES-128
//! (`aes`), AES-CMAC (`cmac`), AES-CCM with an 8-byte tag and a 13-byte nonce
//! (`ccm`), AES-OFB (`ofb`) and AES-CBC-MAC (`cbc-mac`).

use aes::cipher::{BlockCipherEncrypt, KeyInit, KeyIvInit, StreamCipher};
use aes::Aes128;
use ccm::aead::{Aead, Payload};
use ccm::consts::{U13, U8};
use cmac::Mac;

/// AES-128-CCM as S2 uses it: 8-byte tag, 13-byte nonce.
type S2Ccm = ccm::Ccm<Aes128, U8, U13>;

/// Length of the CCM authentication tag S2 appends.
pub const CCM_TAG_LEN: usize = 8;
/// Length of the S2 CCM nonce.
pub const CCM_NONCE_LEN: usize = 13;

/// Encrypts one 16-byte block with AES-128 (ECB).
pub fn aes128_ecb(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let cipher = Aes128::new(key.into());
    let mut b = aes::Block::from(*block);
    cipher.encrypt_block(&mut b);
    b.into()
}

/// AES-128-CMAC (NIST SP 800-38B) of `msg` under `key`.
pub fn cmac(key: &[u8; 16], msg: &[u8]) -> [u8; 16] {
    let mut mac = <cmac::Cmac<Aes128> as KeyInit>::new(key.into());
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

/// AES-128-CCM encryption: returns ciphertext followed by the 8-byte tag.
pub fn ccm_encrypt(key: &[u8; 16], nonce: &[u8; 13], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = S2Ccm::new(key.into());
    cipher
        .encrypt(
            nonce.into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        // CCM with L=2 only fails for messages over 64 KiB, which a Z-Wave
        // frame can never be.
        .expect("CCM input within limits")
}

/// AES-128-CCM decryption of ciphertext-with-tag; `None` when authentication fails.
pub fn ccm_decrypt(
    key: &[u8; 16],
    nonce: &[u8; 13],
    aad: &[u8],
    ciphertext_and_tag: &[u8],
) -> Option<Vec<u8>> {
    if ciphertext_and_tag.len() < CCM_TAG_LEN {
        return None;
    }
    let cipher = S2Ccm::new(key.into());
    cipher
        .decrypt(
            nonce.into(),
            Payload {
                msg: ciphertext_and_tag,
                aad,
            },
        )
        .ok()
}

/// AES-128-OFB keystream applied to `data` (encrypts and decrypts).
pub fn aes128_ofb(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    let mut cipher = ofb::Ofb::<Aes128>::new(key.into(), iv.into());
    cipher.apply_keystream(&mut out);
    out
}

/// AES-128-CBC-MAC with a zero IV and zero padding: the full last block.
pub fn cbc_mac(key: &[u8; 16], data: &[u8]) -> [u8; 16] {
    let mut mac = <cbc_mac::CbcMac<Aes128> as KeyInit>::new(key.into());
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Increments a big-endian counter in place, wrapping.
pub(crate) fn increment(block: &mut [u8]) {
    for b in block.iter_mut().rev() {
        let (v, carry) = b.overflowing_add(1);
        *b = v;
        if !carry {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s.replace(' ', "")).unwrap()
    }
    fn k(s: &str) -> [u8; 16] {
        h(s).try_into().unwrap()
    }

    // FIPS-197 appendix C.1.
    #[test]
    fn aes128_fips197() {
        let key = k("000102030405060708090a0b0c0d0e0f");
        let pt = k("00112233445566778899aabbccddeeff");
        assert_eq!(aes128_ecb(&key, &pt).to_vec(), h("69c4e0d86a7b0430d8cdb78070b4c55a"));
    }

    // RFC 4493 section 4, examples 1 to 4.
    #[test]
    fn cmac_rfc4493() {
        let key = k("2b7e151628aed2a6abf7158809cf4f3c");
        let m = h("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e5130c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710");
        assert_eq!(cmac(&key, &[]).to_vec(), h("bb1d6929e95937287fa37d129b756746"));
        assert_eq!(cmac(&key, &m[..16]).to_vec(), h("070a16b46b4d4144f79bdd9dd04a287c"));
        assert_eq!(cmac(&key, &m[..40]).to_vec(), h("dfa66747de9ae63030ca32611497c827"));
        assert_eq!(cmac(&key, &m).to_vec(), h("51f0bebf7e3b9d92fc49741779363cfe"));
    }

    // RFC 3610 packet vector #1: AES-128, M = 8 (tag), L = 2 (13-byte nonce),
    // the parameters S2 uses.
    #[test]
    fn ccm_rfc3610_vector1() {
        let key = k("c0c1c2c3c4c5c6c7c8c9cacbcccdcecf");
        let nonce: [u8; 13] = h("00000003020100a0a1a2a3a4a5").try_into().unwrap();
        let aad = h("0001020304050607");
        let pt = h("08090a0b0c0d0e0f101112131415161718191a1b1c1d1e");
        let ct = ccm_encrypt(&key, &nonce, &aad, &pt);
        assert_eq!(ct, h("588c979a61c663d2f066d0c2c0f989806d5f6b61dac38417e8d12cfdf926e0"));
        assert_eq!(ccm_decrypt(&key, &nonce, &aad, &ct).unwrap(), pt);
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert!(ccm_decrypt(&key, &nonce, &aad, &bad).is_none());
    }

    // NIST SP 800-38A F.4.1 (OFB-AES128.Encrypt), first two blocks.
    #[test]
    fn ofb_sp800_38a() {
        let key = k("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = k("000102030405060708090a0b0c0d0e0f");
        let pt = h("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        assert_eq!(
            aes128_ofb(&key, &iv, &pt),
            h("3b3fd92eb72dad20333449f8e83cfb4a7789508d16918f03f53c52dac54ed825")
        );
    }

    // CBC-MAC equals the last block of CBC encryption with a zero IV
    // (SP 800-38A F.2.1, CBC-AES128.Encrypt, with that IV's effect removed by
    // XORing it into the first plaintext block).
    #[test]
    fn cbc_mac_matches_cbc() {
        let key = k("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = h("000102030405060708090a0b0c0d0e0f");
        let mut pt = h("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        for (p, i) in pt.iter_mut().zip(iv) {
            *p ^= i;
        }
        assert_eq!(cbc_mac(&key, &pt).to_vec(), h("5086cb9b507219ee95db113a917678b2"));
    }

    // An S0 MAC from captured Z-Wave traffic (published with Z-Wave JS's tests).
    #[test]
    fn s0_mac_real_capture() {
        let key = k("c5fe1ca17d36c992731a0c0c468c1ef9");
        let data = h("ddd360c382a437514392826cbba0b3128114010cf3fb762d6e82126681c18597");
        assert_eq!(cbc_mac(&key, &data)[..8].to_vec(), h("2bc20a8aa9bbb371"));
    }

    #[test]
    fn increment_wraps() {
        let mut b = [0x00, 0xff, 0xff];
        increment(&mut b);
        assert_eq!(b, [0x01, 0x00, 0x00]);
        let mut b = [0xff; 2];
        increment(&mut b);
        assert_eq!(b, [0, 0]);
    }
}
