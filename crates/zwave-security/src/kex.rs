//! S2 key exchange (KEX) constants and the Curve25519 key pair.
//!
//! The exchange itself is a conversation between the controller and the
//! joining node and lives in the host stack; this module holds the pieces
//! both sides compute identically.

use crate::ckdf::{temp_keys, temp_prk, DerivedKeys};
use crate::Error;
use x25519_dalek::{PublicKey, StaticSecret};

/// KEX scheme 1, the bit in the "supported schemes" byte.
pub const KEX_SCHEME_1: u8 = 0x02;
/// The Curve25519 ECDH profile, the bit in the "supported profiles" byte.
pub const ECDH_CURVE25519: u8 = 0x01;

/// Why a key exchange was aborted (the KEX Fail command's payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KexFail {
    /// No supported key is requested or granted.
    Key = 0x01,
    /// No supported KEX scheme.
    Scheme = 0x02,
    /// No supported ECDH curve.
    Curves = 0x03,
    /// A frame could not be decrypted with the temporary key.
    Decrypt = 0x05,
    /// The user or the controller cancelled.
    Cancel = 0x06,
    /// The echoed KEX Set/Report did not match.
    Auth = 0x07,
    /// A key was requested that was not granted.
    KeyGet = 0x08,
    /// The Network Key Verify did not decrypt.
    KeyVerify = 0x09,
    /// The Network Key Report was not valid.
    KeyReport = 0x0a,
}

impl KexFail {
    /// The fail type for a wire value.
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0x01 => KexFail::Key,
            0x02 => KexFail::Scheme,
            0x03 => KexFail::Curves,
            0x05 => KexFail::Decrypt,
            0x06 => KexFail::Cancel,
            0x07 => KexFail::Auth,
            0x08 => KexFail::KeyGet,
            0x09 => KexFail::KeyVerify,
            0x0a => KexFail::KeyReport,
            _ => return None,
        })
    }
}

/// A Curve25519 key pair: the controller's long-term pair, or a node's.
#[derive(Clone)]
pub struct KeyPair {
    secret: StaticSecret,
    public: [u8; 32],
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyPair {{ public: {:02x?} }}", &self.public[..4])
    }
}

impl KeyPair {
    /// A fresh key pair from the operating system's CSPRNG.
    pub fn generate() -> Result<Self, Error> {
        Ok(Self::from_private(crate::random_bytes::<32>()?))
    }

    /// The key pair for a stored private key (clamped as X25519 does).
    pub fn from_private(private: [u8; 32]) -> Self {
        let secret = StaticSecret::from(private);
        let public = PublicKey::from(&secret).to_bytes();
        KeyPair { secret, public }
    }

    /// The private key, for storage.
    pub fn private_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    /// The public key.
    pub fn public(&self) -> &[u8; 32] {
        &self.public
    }

    /// The ECDH shared secret with a peer's public key. Refuses low-order
    /// points, whose all-zero secret anyone could predict.
    pub fn shared_secret(&self, peer_public: &[u8; 32]) -> Result<[u8; 32], Error> {
        let shared = self.secret.diffie_hellman(&PublicKey::from(*peer_public));
        if !shared.was_contributory() {
            return Err(Error::Invalid("public key"));
        }
        Ok(shared.to_bytes())
    }
}

/// The temporary keys both ends derive once public keys are exchanged.
///
/// `controller_public` is the including node's key, `node_public` the
/// joining node's (with its DSK PIN bytes restored).
pub fn bootstrap_keys(
    shared_secret: &[u8; 32],
    controller_public: &[u8; 32],
    node_public: &[u8; 32],
) -> DerivedKeys {
    temp_keys(&temp_prk(shared_secret, controller_public, node_public))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 7748 section 6.1.
    #[test]
    fn x25519_rfc7748() {
        let h = |s: &str| -> [u8; 32] { hex::decode(s).unwrap().try_into().unwrap() };
        let alice = KeyPair::from_private(h(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ));
        let bob = KeyPair::from_private(h(
            "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb",
        ));
        assert_eq!(
            alice.public(),
            &h("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
        );
        assert_eq!(
            bob.public(),
            &h("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f")
        );
        let k = h("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
        assert_eq!(alice.shared_secret(bob.public()).unwrap(), k);
        assert_eq!(bob.shared_secret(alice.public()).unwrap(), k);
    }

    #[test]
    fn low_order_point_refused() {
        let a = KeyPair::generate().unwrap();
        assert!(a.shared_secret(&[0u8; 32]).is_err());
    }

    #[test]
    fn both_ends_derive_the_same_temp_keys() {
        let ctrl = KeyPair::generate().unwrap();
        let node = KeyPair::generate().unwrap();
        let a = bootstrap_keys(
            &ctrl.shared_secret(node.public()).unwrap(),
            ctrl.public(),
            node.public(),
        );
        let b = bootstrap_keys(
            &node.shared_secret(ctrl.public()).unwrap(),
            ctrl.public(),
            node.public(),
        );
        assert_eq!(a, b);
    }
}
