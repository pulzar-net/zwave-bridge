//! Security 0: nonces and Message Encapsulation.
//!
//! S0 frames are AES-128-OFB encrypted and authenticated with an 8-byte
//! AES-CBC-MAC. Every frame needs a fresh 8-byte nonce from the receiver
//! (Nonce Get, Nonce Report), so a sender asks first; the IV is the sender's
//! nonce followed by the receiver's.

use crate::crypto::{aes128_ecb, aes128_ofb, cbc_mac};
use crate::{random_bytes, Error, NetworkKey};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The Security (S0) command class id.
pub const CC_SECURITY: u8 = 0x98;

/// Security 0 command ids.
pub mod cmd {
    /// Commands Supported Get.
    pub const COMMANDS_SUPPORTED_GET: u8 = 0x02;
    /// Commands Supported Report.
    pub const COMMANDS_SUPPORTED_REPORT: u8 = 0x03;
    /// Scheme Get.
    pub const SCHEME_GET: u8 = 0x04;
    /// Scheme Report.
    pub const SCHEME_REPORT: u8 = 0x05;
    /// Network Key Set.
    pub const NETWORK_KEY_SET: u8 = 0x06;
    /// Network Key Verify.
    pub const NETWORK_KEY_VERIFY: u8 = 0x07;
    /// Scheme Inherit.
    pub const SCHEME_INHERIT: u8 = 0x08;
    /// Nonce Get.
    pub const NONCE_GET: u8 = 0x40;
    /// Nonce Report.
    pub const NONCE_REPORT: u8 = 0x80;
    /// Message Encapsulation.
    pub const MESSAGE_ENCAPSULATION: u8 = 0x81;
    /// Message Encapsulation, asking for a nonce in the same breath.
    pub const MESSAGE_ENCAPSULATION_NONCE_GET: u8 = 0xc1;
}

/// How long a nonce we issued stays valid.
pub const NONCE_LIFETIME: Duration = Duration::from_secs(10);
/// Most nonces we keep outstanding at once.
pub const MAX_OWN_NONCES: usize = 64;
/// The longest S0 payload in one frame; longer commands are split in two.
pub const MAX_PAYLOAD: usize = 26;

/// The keys derived from one S0 network key.
#[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
struct Keys {
    auth: [u8; 16],
    enc: [u8; 16],
}

impl Keys {
    fn new(network_key: &NetworkKey) -> Self {
        Keys {
            auth: aes128_ecb(network_key, &[0x55; 16]),
            enc: aes128_ecb(network_key, &[0xaa; 16]),
        }
    }
}

/// The well-known all-zero key S0 uses while handing over the real one.
pub const TEMP_KEY: NetworkKey = [0; 16];

/// A decapsulated S0 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S0Payload {
    /// A complete command (command class id first).
    Complete(Vec<u8>),
    /// The first half of a two-frame command; the second follows.
    FirstHalf,
}

/// Security 0 state for one node.
pub struct S0Manager {
    own_node: u16,
    keys: Keys,
    temp: Keys,
    own_nonces: HashMap<u8, ([u8; 8], u16, Instant)>,
    peer_nonces: HashMap<u16, ([u8; 8], Instant)>,
    halves: HashMap<(u16, u8), (Vec<u8>, Instant)>,
    seq: u8,
}

impl std::fmt::Debug for S0Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S0Manager")
            .field("own_node", &self.own_node)
            .finish()
    }
}

/// One frame of an outgoing encapsulation and whether it asks for the next nonce.
fn frame_control(sequenced: bool, second: bool, counter: u8) -> u8 {
    (if sequenced { 0x10 } else { 0 }) | (if second { 0x20 } else { 0 }) | (counter & 0x0f)
}

impl S0Manager {
    /// A manager for `own_node` with the network key.
    pub fn new(own_node: u16, network_key: &NetworkKey) -> Self {
        S0Manager {
            own_node,
            keys: Keys::new(network_key),
            temp: Keys::new(&TEMP_KEY),
            own_nonces: HashMap::new(),
            peer_nonces: HashMap::new(),
            halves: HashMap::new(),
            seq: 0,
        }
    }

    /// Changes the own node id (after being included).
    pub fn set_own_node(&mut self, own_node: u16) {
        self.own_node = own_node;
    }

    /// Replaces the network key (a node receiving Network Key Set).
    pub fn set_network_key(&mut self, key: &NetworkKey) {
        self.keys = Keys::new(key);
    }

    fn expire(&mut self, now: Instant) {
        self.own_nonces
            .retain(|_, (_, _, t)| now.duration_since(*t) < NONCE_LIFETIME);
        self.peer_nonces
            .retain(|_, (_, t)| now.duration_since(*t) < NONCE_LIFETIME);
        self.halves
            .retain(|_, (_, t)| now.duration_since(*t) < NONCE_LIFETIME);
    }

    /// A Nonce Get command.
    pub fn nonce_get() -> Vec<u8> {
        vec![CC_SECURITY, cmd::NONCE_GET]
    }

    /// Issues a nonce for `peer` and returns the Nonce Report carrying it.
    pub fn nonce_report(&mut self, peer: u16) -> Result<Vec<u8>, Error> {
        let now = Instant::now();
        self.expire(now);
        if self.own_nonces.len() >= MAX_OWN_NONCES {
            // Drop the oldest; a flood of Nonce Gets must not grow memory.
            if let Some(oldest) = self
                .own_nonces
                .iter()
                .min_by_key(|(_, (_, _, t))| *t)
                .map(|(k, _)| *k)
            {
                self.own_nonces.remove(&oldest);
            }
        }
        let nonce = loop {
            let n = random_bytes::<8>()?;
            // The first byte is the nonce id and must be unique among live nonces.
            if !self.own_nonces.contains_key(&n[0]) {
                break n;
            }
        };
        self.own_nonces.insert(nonce[0], (nonce, peer, now));
        let mut v = vec![CC_SECURITY, cmd::NONCE_REPORT];
        v.extend_from_slice(&nonce);
        Ok(v)
    }

    /// Stores the nonce from a peer's Nonce Report (`frame` starts at `0x98 0x80`).
    pub fn handle_nonce_report(&mut self, peer: u16, frame: &[u8]) -> Result<(), Error> {
        if frame.len() != 10 || frame[0] != CC_SECURITY || frame[1] != cmd::NONCE_REPORT {
            return Err(Error::Malformed("not an S0 nonce report"));
        }
        let n: [u8; 8] = frame[2..10].try_into().expect("8 bytes");
        self.peer_nonces.insert(peer, (n, Instant::now()));
        Ok(())
    }

    /// Whether we hold a live nonce from `peer`.
    pub fn has_peer_nonce(&self, peer: u16) -> bool {
        self.peer_nonces
            .get(&peer)
            .is_some_and(|(_, t)| t.elapsed() < NONCE_LIFETIME)
    }

    /// Encapsulates `command` for `peer` with its stored nonce (consumed).
    ///
    /// A command longer than [`MAX_PAYLOAD`] needs two frames; this returns
    /// the first and the caller sends it, fetches a new nonce and calls
    /// again with the rest via [`encapsulate_part`](Self::encapsulate_part).
    pub fn encapsulate(
        &mut self,
        peer: u16,
        command: &[u8],
        temp_key: bool,
    ) -> Result<Vec<u8>, Error> {
        if command.len() > MAX_PAYLOAD {
            return Err(Error::Malformed("S0 command too long for one frame"));
        }
        self.encapsulate_part(
            peer,
            command,
            frame_control(false, false, 0),
            temp_key,
            false,
        )
    }

    /// Splits a long command into the two halves S0 sends in sequence.
    pub fn split(&mut self, command: &[u8]) -> Option<[(Vec<u8>, u8); 2]> {
        if command.len() <= MAX_PAYLOAD || command.len() > 2 * MAX_PAYLOAD {
            return None;
        }
        self.seq = (self.seq + 1) & 0x0f;
        let (a, b) = command.split_at(MAX_PAYLOAD);
        Some([
            (a.to_vec(), frame_control(true, false, self.seq)),
            (b.to_vec(), frame_control(true, true, self.seq)),
        ])
    }

    /// Encapsulates one part with an explicit frame control byte. `nonce_get`
    /// asks the receiver for its next nonce in the same frame.
    pub fn encapsulate_part(
        &mut self,
        peer: u16,
        part: &[u8],
        control: u8,
        temp_key: bool,
        nonce_get: bool,
    ) -> Result<Vec<u8>, Error> {
        let now = Instant::now();
        self.expire(now);
        let (receiver_nonce, _) = self.peer_nonces.remove(&peer).ok_or(Error::NoNonce)?;
        let sender_nonce = random_bytes::<8>()?;
        let keys = if temp_key { &self.temp } else { &self.keys };
        let mut iv = [0u8; 16];
        iv[..8].copy_from_slice(&sender_nonce);
        iv[8..].copy_from_slice(&receiver_nonce);
        let mut plain = Vec::with_capacity(part.len() + 1);
        plain.push(control);
        plain.extend_from_slice(part);
        let ct = aes128_ofb(&keys.enc, &iv, &plain);
        let command = if nonce_get {
            cmd::MESSAGE_ENCAPSULATION_NONCE_GET
        } else {
            cmd::MESSAGE_ENCAPSULATION
        };
        let mac = Self::mac(&keys.auth, &iv, command, self.own_node, peer, &ct);
        let mut out = Vec::with_capacity(2 + 8 + ct.len() + 9);
        out.extend_from_slice(&[CC_SECURITY, command]);
        out.extend_from_slice(&sender_nonce);
        out.extend_from_slice(&ct);
        out.push(receiver_nonce[0]);
        out.extend_from_slice(&mac);
        Ok(out)
    }

    fn mac(
        auth: &[u8; 16],
        iv: &[u8; 16],
        command: u8,
        sender: u16,
        receiver: u16,
        ct: &[u8],
    ) -> [u8; 8] {
        let mut data = Vec::with_capacity(20 + ct.len());
        data.extend_from_slice(iv);
        data.extend_from_slice(&[command, sender as u8, receiver as u8, ct.len() as u8]);
        data.extend_from_slice(ct);
        let full = cbc_mac(auth, &data);
        full[..8].try_into().expect("8 bytes")
    }

    /// Decapsulates a frame from `peer` (`frame` starts at `0x98 0x81` or `0x98 0xC1`).
    ///
    /// The nonce it names must be one we issued to `peer` and still live; it
    /// is consumed either way.
    pub fn decapsulate(
        &mut self,
        peer: u16,
        frame: &[u8],
        temp_key: bool,
    ) -> Result<S0Payload, Error> {
        if frame.len() < 2 + 8 + 1 + 1 + 8
            || frame[0] != CC_SECURITY
            || !matches!(
                frame[1],
                cmd::MESSAGE_ENCAPSULATION | cmd::MESSAGE_ENCAPSULATION_NONCE_GET
            )
        {
            return Err(Error::Malformed("not an S0 encapsulation"));
        }
        let now = Instant::now();
        self.expire(now);
        let sender_nonce = &frame[2..10];
        let ct = &frame[10..frame.len() - 9];
        let nonce_id = frame[frame.len() - 9];
        let mac = &frame[frame.len() - 8..];
        let (receiver_nonce, issued_to, _) =
            self.own_nonces.remove(&nonce_id).ok_or(Error::NoNonce)?;
        if issued_to != peer {
            return Err(Error::NoNonce);
        }
        let keys = if temp_key { &self.temp } else { &self.keys };
        let mut iv = [0u8; 16];
        iv[..8].copy_from_slice(sender_nonce);
        iv[8..].copy_from_slice(&receiver_nonce);
        let want = Self::mac(&keys.auth, &iv, frame[1], peer, self.own_node, ct);
        if want.as_slice() != mac {
            return Err(Error::DecryptFailed);
        }
        let plain = aes128_ofb(&keys.enc, &iv, ct);
        let control = plain[0];
        let body = plain[1..].to_vec();
        let sequenced = control & 0x10 != 0;
        let second = control & 0x20 != 0;
        let counter = control & 0x0f;
        if !sequenced {
            return Ok(S0Payload::Complete(body));
        }
        if !second {
            self.halves.insert((peer, counter), (body, now));
            return Ok(S0Payload::FirstHalf);
        }
        let (mut first, _) = self
            .halves
            .remove(&(peer, counter))
            .ok_or(Error::Malformed("S0 second half without first"))?;
        first.extend_from_slice(&body);
        Ok(S0Payload::Complete(first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = [7u8; 16];
        let mut c = S0Manager::new(1, &key);
        let mut n = S0Manager::new(9, &key);
        assert_eq!(c.encapsulate(9, &[0x25, 0x02], false), Err(Error::NoNonce));
        let report = n.nonce_report(1).unwrap();
        c.handle_nonce_report(9, &report).unwrap();
        let f = c.encapsulate(9, &[0x25, 0x01, 0xff], false).unwrap();
        assert_eq!(
            n.decapsulate(1, &f, false).unwrap(),
            S0Payload::Complete(vec![0x25, 0x01, 0xff])
        );
        // The nonce was single-use.
        assert_eq!(n.decapsulate(1, &f, false), Err(Error::NoNonce));
    }

    #[test]
    fn mac_and_key_are_checked() {
        let mut c = S0Manager::new(1, &[7; 16]);
        let mut n = S0Manager::new(9, &[8; 16]);
        let r = n.nonce_report(1).unwrap();
        c.handle_nonce_report(9, &r).unwrap();
        let f = c.encapsulate(9, &[0x25, 0x02], false).unwrap();
        assert_eq!(n.decapsulate(1, &f, false), Err(Error::DecryptFailed));
        // Temporary key during key handover.
        let r = n.nonce_report(1).unwrap();
        c.handle_nonce_report(9, &r).unwrap();
        let mut set = vec![CC_SECURITY, cmd::NETWORK_KEY_SET];
        set.extend_from_slice(&[7; 16]);
        let f = c.encapsulate(9, &set, true).unwrap();
        assert_eq!(
            n.decapsulate(1, &f, true).unwrap(),
            S0Payload::Complete(set)
        );
    }

    #[test]
    fn nonce_from_another_peer_is_refused() {
        let key = [7u8; 16];
        let mut c = S0Manager::new(1, &key);
        let mut n = S0Manager::new(9, &key);
        let r = n.nonce_report(2).unwrap();
        c.handle_nonce_report(9, &r).unwrap();
        let f = c.encapsulate(9, &[0x20, 0x02], false).unwrap();
        assert_eq!(n.decapsulate(1, &f, false), Err(Error::NoNonce));
    }

    #[test]
    fn two_part_commands() {
        let key = [7u8; 16];
        let mut c = S0Manager::new(1, &key);
        let mut n = S0Manager::new(9, &key);
        let long: Vec<u8> = (0..40).collect();
        let parts = c.split(&long).unwrap();
        for (i, (part, control)) in parts.iter().enumerate() {
            let r = n.nonce_report(1).unwrap();
            c.handle_nonce_report(9, &r).unwrap();
            let f = c
                .encapsulate_part(9, part, *control, false, i == 0)
                .unwrap();
            let out = n.decapsulate(1, &f, false).unwrap();
            if i == 0 {
                assert_eq!(out, S0Payload::FirstHalf);
            } else {
                assert_eq!(out, S0Payload::Complete(long.clone()));
            }
        }
    }

    #[test]
    fn nonce_table_is_bounded() {
        let mut n = S0Manager::new(9, &[0; 16]);
        for _ in 0..500 {
            n.nonce_report(1).unwrap();
        }
        assert!(n.own_nonces.len() <= MAX_OWN_NONCES);
    }
}
