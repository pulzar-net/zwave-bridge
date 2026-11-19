//! AES-128 CTR_DRBG without a derivation function (NIST SP 800-90A Rev. 1,
//! section 10.2.1), the generator S2 uses for SPAN nonces.
//!
//! Both ends of a SPAN seed one of these with the same mixed entropy input
//! and personalization string and then draw nonces in lockstep, so it must be
//! bit-exact; it is tested against NIST's CAVP vectors. The AES block
//! function is the `aes` crate's.

use crate::crypto::{aes128_ecb, increment};

/// Seed length: key length plus block length.
pub const SEED_LEN: usize = 32;

/// AES-128 CTR_DRBG state (no derivation function, no prediction resistance).
#[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct CtrDrbg {
    key: [u8; 16],
    v: [u8; 16],
}

impl std::fmt::Debug for CtrDrbg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CtrDrbg(..)")
    }
}

impl CtrDrbg {
    /// Instantiates from 32 bytes of entropy and an optional personalization
    /// string of at most 32 bytes (shorter strings are zero padded).
    pub fn new(entropy: &[u8; SEED_LEN], personalization: &[u8]) -> Self {
        assert!(personalization.len() <= SEED_LEN, "personalization too long");
        let mut seed = *entropy;
        for (s, p) in seed.iter_mut().zip(personalization) {
            *s ^= p;
        }
        let mut d = CtrDrbg {
            key: [0; 16],
            v: [0; 16],
        };
        d.update(Some(&seed));
        d
    }

    fn update(&mut self, provided: Option<&[u8; SEED_LEN]>) {
        let mut temp = [0u8; SEED_LEN];
        for chunk in temp.chunks_mut(16) {
            increment(&mut self.v);
            chunk.copy_from_slice(&aes128_ecb(&self.key, &self.v));
        }
        if let Some(p) = provided {
            for (t, p) in temp.iter_mut().zip(p) {
                *t ^= p;
            }
        }
        self.key.copy_from_slice(&temp[..16]);
        self.v.copy_from_slice(&temp[16..]);
    }

    /// Reseeds with 32 bytes of new entropy and optional additional input.
    pub fn reseed(&mut self, entropy: &[u8; SEED_LEN], additional: &[u8]) {
        assert!(additional.len() <= SEED_LEN, "additional input too long");
        let mut seed = *entropy;
        for (s, a) in seed.iter_mut().zip(additional) {
            *s ^= a;
        }
        self.update(Some(&seed));
    }

    /// Fills `out` with output and advances the state (no additional input).
    pub fn generate(&mut self, out: &mut [u8]) {
        self.generate_with(out, &[]);
    }

    /// Fills `out` with output, mixing in up to 32 bytes of additional input.
    pub fn generate_with(&mut self, out: &mut [u8], additional: &[u8]) {
        assert!(additional.len() <= SEED_LEN, "additional input too long");
        let add = if additional.is_empty() {
            None
        } else {
            let mut a = [0u8; SEED_LEN];
            a[..additional.len()].copy_from_slice(additional);
            self.update(Some(&a));
            Some(a)
        };
        for chunk in out.chunks_mut(16) {
            increment(&mut self.v);
            let block = aes128_ecb(&self.key, &self.v);
            chunk.copy_from_slice(&block[..chunk.len()]);
        }
        self.update(add.as_ref());
    }

    /// Draws one 16-byte block and returns its first 13 bytes: an S2 nonce.
    pub fn next_nonce(&mut self) -> [u8; 13] {
        let mut block = [0u8; 16];
        self.generate(&mut block);
        let mut n = [0u8; 13];
        n.copy_from_slice(&block[..13]);
        n
    }
}
