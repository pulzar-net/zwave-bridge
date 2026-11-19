//! Security 2: Message Encapsulation, extensions, and the SPAN and MPAN
//! nonce state.
//!
//! A [`S2Manager`] holds the network keys, the per-peer singlecast state
//! (SPAN) and the multicast state (MPAN) for one node, which is the
//! controller in a host stack and the simulated node in a simulator. It
//! builds and consumes the Security 2 frames whose contents are crypto (Nonce
//! Get, Nonce Report, Message Encapsulation); the key exchange commands are
//! ordinary command classes.
//!
//! Nonces: the receiver sends a Nonce Report carrying its receiver entropy
//! input (REI) when it cannot decrypt, the sender answers with its sender
//! entropy input (SEI) in a SPAN extension, and from then on both ends draw
//! nonces in lockstep from a CTR_DRBG seeded by both inputs and the key's
//! personalization string.

use crate::ckdf::{mei, network_keys, nonce_prk, DerivedKeys};
use crate::crypto::{aes128_ecb, ccm_decrypt, ccm_encrypt, increment, CCM_TAG_LEN};
use crate::drbg::CtrDrbg;
use crate::{random_bytes, Error, NetworkKey, SecurityClass};
use std::collections::{BTreeMap, HashMap};

/// The Security 2 command class id.
pub const CC_SECURITY_2: u8 = 0x9f;

/// Security 2 command ids.
pub mod cmd {
    /// Nonce Get.
    pub const NONCE_GET: u8 = 0x01;
    /// Nonce Report.
    pub const NONCE_REPORT: u8 = 0x02;
    /// Message Encapsulation.
    pub const MESSAGE_ENCAPSULATION: u8 = 0x03;
    /// KEX Get.
    pub const KEX_GET: u8 = 0x04;
    /// KEX Report.
    pub const KEX_REPORT: u8 = 0x05;
    /// KEX Set.
    pub const KEX_SET: u8 = 0x06;
    /// KEX Fail.
    pub const KEX_FAIL: u8 = 0x07;
    /// Public Key Report.
    pub const PUBLIC_KEY_REPORT: u8 = 0x08;
    /// Network Key Get.
    pub const NETWORK_KEY_GET: u8 = 0x09;
    /// Network Key Report.
    pub const NETWORK_KEY_REPORT: u8 = 0x0a;
    /// Network Key Verify.
    pub const NETWORK_KEY_VERIFY: u8 = 0x0b;
    /// Transfer End.
    pub const TRANSFER_END: u8 = 0x0c;
    /// Commands Supported Get.
    pub const COMMANDS_SUPPORTED_GET: u8 = 0x0d;
    /// Commands Supported Report.
    pub const COMMANDS_SUPPORTED_REPORT: u8 = 0x0e;
}

/// How many successive SPAN nonces a receiver tries before asking for a resync.
pub const MAX_DECRYPT_ATTEMPTS: usize = 5;

/// Which key a frame was (or is to be) protected with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyId {
    /// The temporary key of a key exchange in progress.
    Temporary,
    /// A network key.
    Class(SecurityClass),
}

/// A Security 2 Message Encapsulation extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extension {
    /// Singlecast PAN: the sender's entropy input.
    Span {
        /// Sender entropy input.
        sender_ei: [u8; 16],
    },
    /// Multicast PAN: a group's inner state (always encrypted).
    Mpan {
        /// Multicast group id.
        group: u8,
        /// The group's inner MPAN state.
        inner_state: [u8; 16],
    },
    /// Multicast group: this frame belongs to (or follows up) group `group`.
    Mgrp {
        /// Multicast group id.
        group: u8,
    },
    /// Multicast out of sync: the sender lacks an MPAN of ours.
    Mos,
    /// An extension this implementation does not know (non-critical only).
    Unknown {
        /// Extension type.
        kind: u8,
        /// Payload.
        data: Vec<u8>,
    },
}

impl Extension {
    fn kind(&self) -> u8 {
        match self {
            Extension::Span { .. } => 0x01,
            Extension::Mpan { .. } => 0x02,
            Extension::Mgrp { .. } => 0x03,
            Extension::Mos => 0x04,
            Extension::Unknown { kind, .. } => *kind,
        }
    }

    fn critical(&self) -> bool {
        matches!(
            self,
            Extension::Span { .. } | Extension::Mpan { .. } | Extension::Mgrp { .. }
        )
    }

    fn is_encrypted(&self) -> bool {
        matches!(self, Extension::Mpan { .. })
    }

    fn encode(&self, more: bool, out: &mut Vec<u8>) {
        let payload: Vec<u8> = match self {
            Extension::Span { sender_ei } => sender_ei.to_vec(),
            Extension::Mpan { group, inner_state } => {
                let mut v = vec![*group];
                v.extend_from_slice(inner_state);
                v
            }
            Extension::Mgrp { group } => vec![*group],
            Extension::Mos => vec![],
            Extension::Unknown { data, .. } => data.clone(),
        };
        out.push((2 + payload.len()) as u8);
        out.push(
            if more { 0x80 } else { 0 } | if self.critical() { 0x40 } else { 0 } | (self.kind() & 0x3f),
        );
        out.extend_from_slice(&payload);
    }
}

/// Parses a chain of extensions at the start of `data`; returns them and the bytes used.
fn parse_extensions(data: &[u8]) -> Result<(Vec<Extension>, usize), Error> {
    let mut exts = Vec::new();
    let mut at = 0;
    loop {
        let len = *data.get(at).ok_or(Error::Malformed("extension length"))? as usize;
        let head = *data.get(at + 1).ok_or(Error::Malformed("extension header"))?;
        if len < 2 || at + len > data.len() {
            return Err(Error::Malformed("extension length"));
        }
        let body = &data[at + 2..at + len];
        let more = head & 0x80 != 0;
        let critical = head & 0x40 != 0;
        let kind = head & 0x3f;
        let ext = match (kind, body.len()) {
            (0x01, 16) => Extension::Span {
                sender_ei: body.try_into().expect("16 bytes"),
            },
            (0x02, 17) => Extension::Mpan {
                group: body[0],
                inner_state: body[1..].try_into().expect("16 bytes"),
            },
            (0x03, 1) => Extension::Mgrp { group: body[0] },
            (0x04, 0) => Extension::Mos,
            (0x01..=0x04, _) => return Err(Error::Malformed("extension size")),
            _ if critical => return Err(Error::UnsupportedExtension(kind)),
            _ => Extension::Unknown {
                kind,
                data: body.to_vec(),
            },
        };
        exts.push(ext);
        at += len;
        if !more {
            return Ok((exts, at));
        }
    }
}

fn encode_extensions(exts: &[&Extension], out: &mut Vec<u8>) {
    for (i, e) in exts.iter().enumerate() {
        e.encode(i + 1 < exts.len(), out);
    }
}

/// The unencrypted parts of a Message Encapsulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encapsulation {
    /// The sender's sequence number.
    pub sequence: u8,
    /// Unencrypted extensions.
    pub extensions: Vec<Extension>,
    /// Whether encrypted extensions precede the command in the plaintext.
    pub encrypted_extensions: bool,
    /// Bytes of `[sequence, flags, extensions]`: the part of the AAD after the length.
    header_len: usize,
}

impl Encapsulation {
    /// Parses a Message Encapsulation command (starting at `0x9F 0x03`).
    pub fn parse(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() < 4 + CCM_TAG_LEN
            || frame[0] != CC_SECURITY_2
            || frame[1] != cmd::MESSAGE_ENCAPSULATION
        {
            return Err(Error::Malformed("not an S2 encapsulation"));
        }
        let sequence = frame[2];
        let flags = frame[3];
        let (extensions, used) = if flags & 0x01 != 0 {
            parse_extensions(&frame[4..])?
        } else {
            (Vec::new(), 0)
        };
        let header_len = 2 + used;
        if 2 + header_len + CCM_TAG_LEN > frame.len() {
            return Err(Error::Malformed("S2 encapsulation too short"));
        }
        Ok(Encapsulation {
            sequence,
            extensions,
            encrypted_extensions: flags & 0x02 != 0,
            header_len,
        })
    }

    /// The SPAN extension's sender entropy input, if present.
    pub fn sender_ei(&self) -> Option<[u8; 16]> {
        self.extensions.iter().find_map(|e| match e {
            Extension::Span { sender_ei } => Some(*sender_ei),
            _ => None,
        })
    }

    /// The MGRP extension's group, if present.
    pub fn multicast_group(&self) -> Option<u8> {
        self.extensions.iter().find_map(|e| match e {
            Extension::Mgrp { group } => Some(*group),
            _ => None,
        })
    }
}

fn node_bytes(id: u16, long: bool, out: &mut Vec<u8>) {
    if long {
        out.extend_from_slice(&id.to_be_bytes());
    } else {
        out.push(id as u8);
    }
}

fn aad(sender: u16, dest: u16, home_id: u32, total_len: usize, header: &[u8]) -> Vec<u8> {
    let long = sender > 0xff || dest > 0xff;
    let mut a = Vec::with_capacity(10 + header.len());
    node_bytes(sender, long, &mut a);
    node_bytes(dest, long, &mut a);
    a.extend_from_slice(&home_id.to_be_bytes());
    a.extend_from_slice(&(total_len as u16).to_be_bytes());
    a.extend_from_slice(header);
    a
}

/// A Nonce Report as received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonceReport {
    /// Singlecast out of sync: the peer gave us its REI.
    pub sos: bool,
    /// Multicast out of sync: the peer lacks one of our MPANs.
    pub mos: bool,
}

/// A decrypted Message Encapsulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decrypted {
    /// The encapsulated command (command class id first).
    pub command: Vec<u8>,
    /// The key that authenticated it.
    pub key: KeyId,
    /// The sender's sequence number.
    pub sequence: u8,
    /// Unencrypted and encrypted extensions.
    pub extensions: Vec<Extension>,
}

impl Decrypted {
    /// Whether the sender asked for our MPAN (an MOS extension).
    pub fn wants_mpan(&self) -> bool {
        self.extensions.contains(&Extension::Mos)
    }
}

enum Span {
    /// We sent our REI in a Nonce Report and wait for the peer's SEI.
    LocalEi([u8; 16]),
    /// The peer sent us its REI; our next frame carries our SEI.
    RemoteEi([u8; 16]),
    /// Both ends hold the same generator.
    Established { key: KeyId, drbg: CtrDrbg },
}

/// Options for one outgoing encapsulation.
#[derive(Debug, Clone, Default)]
pub struct EncryptOptions {
    /// Force a key; by default the temporary key while bootstrapping, else the peer's highest class.
    pub key: Option<KeyId>,
    /// A multicast follow-up: the MGRP group this singlecast follows.
    pub follow_up_group: Option<u8>,
    /// Hand the peer our MPAN for this group (an encrypted MPAN extension).
    pub send_mpan: Option<u8>,
    /// Tell the peer we lack its MPAN (an MOS extension).
    pub mos: bool,
}

/// Security 2 state for one node: keys, SPANs, MPANs and sequence numbers.
pub struct S2Manager {
    own_node: u16,
    home_id: u32,
    keys: BTreeMap<SecurityClass, DerivedKeys>,
    temp: HashMap<u16, DerivedKeys>,
    spans: HashMap<u16, Span>,
    peer_classes: HashMap<u16, Vec<SecurityClass>>,
    tx_seq: HashMap<u16, u8>,
    rx_seq: HashMap<u16, u8>,
    groups: HashMap<u8, (SecurityClass, [u8; 16])>,
    peer_mpans: HashMap<(u16, u8), [u8; 16]>,
}

impl std::fmt::Debug for S2Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S2Manager")
            .field("own_node", &self.own_node)
            .field("home_id", &format_args!("{:08x}", self.home_id))
            .field("classes", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl S2Manager {
    /// A manager for `own_node` in network `home_id`, with no keys yet.
    pub fn new(own_node: u16, home_id: u32) -> Self {
        S2Manager {
            own_node,
            home_id,
            keys: BTreeMap::new(),
            temp: HashMap::new(),
            spans: HashMap::new(),
            peer_classes: HashMap::new(),
            tx_seq: HashMap::new(),
            rx_seq: HashMap::new(),
            groups: HashMap::new(),
            peer_mpans: HashMap::new(),
        }
    }

    /// Changes the own node id and home id (a node that was just included);
    /// drops every SPAN and MPAN.
    pub fn set_home(&mut self, own_node: u16, home_id: u32) {
        self.own_node = own_node;
        self.home_id = home_id;
        self.spans.clear();
        self.groups.clear();
        self.peer_mpans.clear();
    }

    /// The own node id.
    pub fn own_node(&self) -> u16 {
        self.own_node
    }

    /// The home id.
    pub fn home_id(&self) -> u32 {
        self.home_id
    }

    /// Installs the network key for a class (derives its CCM, nonce and MPAN keys).
    pub fn set_network_key(&mut self, class: SecurityClass, key: &NetworkKey) {
        self.keys.insert(class, network_keys(key));
    }

    /// Whether a key is installed for this class.
    pub fn has_key(&self, class: SecurityClass) -> bool {
        self.keys.contains_key(&class)
    }

    /// Records the classes a peer was granted (any order).
    pub fn set_peer_classes(&mut self, peer: u16, classes: &[SecurityClass]) {
        let mut c: Vec<SecurityClass> = classes.iter().copied().filter(|c| c.is_s2() || *c == SecurityClass::S0Legacy).collect();
        c.sort_by(|a, b| b.cmp(a));
        c.dedup();
        self.peer_classes.insert(peer, c);
    }

    /// The highest S2 class a peer was granted.
    pub fn highest_class(&self, peer: u16) -> Option<SecurityClass> {
        self.peer_classes
            .get(&peer)?
            .iter()
            .copied()
            .find(|c| c.is_s2() && self.keys.contains_key(c))
    }

    /// Forgets a peer entirely (excluded or reset).
    pub fn forget_peer(&mut self, peer: u16) {
        self.temp.remove(&peer);
        self.spans.remove(&peer);
        self.peer_classes.remove(&peer);
        self.tx_seq.remove(&peer);
        self.rx_seq.remove(&peer);
        self.peer_mpans.retain(|(p, _), _| *p != peer);
    }

    /// Starts using the temporary key with `peer` (a key exchange in progress).
    pub fn begin_bootstrap(&mut self, peer: u16, temp: DerivedKeys) {
        self.temp.insert(peer, temp);
        self.spans.remove(&peer);
    }

    /// Ends the key exchange with `peer` and drops its SPAN.
    pub fn end_bootstrap(&mut self, peer: u16) {
        self.temp.remove(&peer);
        self.spans.remove(&peer);
    }

    /// Whether a key exchange with `peer` is in progress.
    pub fn is_bootstrapping(&self, peer: u16) -> bool {
        self.temp.contains_key(&peer)
    }

    /// Drops the SPAN with `peer`; the next exchange needs a nonce.
    pub fn reset_span(&mut self, peer: u16) {
        self.spans.remove(&peer);
    }

    /// Whether a frame to `peer` can be encrypted right now (with the default key).
    pub fn can_encrypt(&self, peer: u16) -> bool {
        match self.spans.get(&peer) {
            Some(Span::RemoteEi(_)) => true,
            Some(Span::Established { key, .. }) => self.default_key(peer) == Some(*key),
            _ => false,
        }
    }

    fn default_key(&self, peer: u16) -> Option<KeyId> {
        if self.temp.contains_key(&peer) {
            Some(KeyId::Temporary)
        } else {
            self.highest_class(peer).map(KeyId::Class)
        }
    }

    fn keys_for(&self, peer: u16, key: KeyId) -> Result<&DerivedKeys, Error> {
        match key {
            KeyId::Temporary => self.temp.get(&peer).ok_or(Error::NoKey(SecurityClass::S2Unauthenticated)),
            KeyId::Class(c) => self.keys.get(&c).ok_or(Error::NoKey(c)),
        }
    }

    fn next_sequence(&mut self, peer: u16) -> Result<u8, Error> {
        let seq = match self.tx_seq.get(&peer) {
            Some(s) => s.wrapping_add(1),
            None => random_bytes::<1>()?[0],
        };
        self.tx_seq.insert(peer, seq);
        Ok(seq)
    }

    /// Builds a Nonce Get for `peer`.
    pub fn nonce_get(&mut self, peer: u16) -> Result<Vec<u8>, Error> {
        let seq = self.next_sequence(peer)?;
        Ok(vec![CC_SECURITY_2, cmd::NONCE_GET, seq])
    }

    /// Builds a Nonce Report with a fresh REI (SOS), optionally also flagging MOS.
    /// Any SPAN with the peer is replaced.
    pub fn nonce_report(&mut self, peer: u16, mos: bool) -> Result<Vec<u8>, Error> {
        let seq = self.next_sequence(peer)?;
        let rei = random_bytes::<16>()?;
        self.spans.insert(peer, Span::LocalEi(rei));
        let mut v = vec![CC_SECURITY_2, cmd::NONCE_REPORT, seq, 0x01 | if mos { 0x02 } else { 0 }];
        v.extend_from_slice(&rei);
        Ok(v)
    }

    /// Builds a Nonce Report that only flags MOS (no new REI).
    pub fn nonce_report_mos(&mut self, peer: u16) -> Result<Vec<u8>, Error> {
        let seq = self.next_sequence(peer)?;
        Ok(vec![CC_SECURITY_2, cmd::NONCE_REPORT, seq, 0x02])
    }

    /// Handles a received Nonce Report (`frame` starts at `0x9F 0x02`).
    pub fn handle_nonce_report(&mut self, peer: u16, frame: &[u8]) -> Result<NonceReport, Error> {
        if frame.len() < 4 || frame[0] != CC_SECURITY_2 || frame[1] != cmd::NONCE_REPORT {
            return Err(Error::Malformed("not a nonce report"));
        }
        let flags = frame[3];
        let sos = flags & 0x01 != 0;
        let mos = flags & 0x02 != 0;
        if sos {
            let rei: [u8; 16] = frame
                .get(4..20)
                .ok_or(Error::Malformed("nonce report without REI"))?
                .try_into()
                .expect("16 bytes");
            self.spans.insert(peer, Span::RemoteEi(rei));
        }
        Ok(NonceReport { sos, mos })
    }

    fn span_drbg(&self, peer: u16, key: KeyId, sei: &[u8; 16], rei: &[u8; 16]) -> Result<CtrDrbg, Error> {
        let keys = self.keys_for(peer, key)?;
        Ok(CtrDrbg::new(&mei(&nonce_prk(sei, rei)), &keys.personalization))
    }

    /// Encrypts `command` (command class id first) for `peer`.
    ///
    /// Fails with [`Error::NoSpan`] when no nonce is shared yet; send
    /// [`nonce_get`](Self::nonce_get), feed the answer to
    /// [`handle_nonce_report`](Self::handle_nonce_report) and retry.
    pub fn encrypt(&mut self, peer: u16, command: &[u8], opts: &EncryptOptions) -> Result<Vec<u8>, Error> {
        let key = match opts.key {
            Some(k) => k,
            None => self.default_key(peer).ok_or(Error::NoKey(SecurityClass::S2Unauthenticated))?,
        };
        let keys = self.keys_for(peer, key)?.clone();
        let mut unencrypted: Vec<Extension> = Vec::new();
        let nonce = match self.spans.remove(&peer) {
            Some(Span::Established { key: k, mut drbg }) if k == key => {
                let n = drbg.next_nonce();
                self.spans.insert(peer, Span::Established { key, drbg });
                n
            }
            Some(Span::RemoteEi(rei)) => {
                let sei = random_bytes::<16>()?;
                let mut drbg = self.span_drbg(peer, key, &sei, &rei)?;
                let n = drbg.next_nonce();
                self.spans.insert(peer, Span::Established { key, drbg });
                unencrypted.push(Extension::Span { sender_ei: sei });
                n
            }
            other => {
                if let Some(s) = other {
                    self.spans.insert(peer, s);
                }
                return Err(Error::NoSpan);
            }
        };
        if let Some(group) = opts.follow_up_group {
            unencrypted.push(Extension::Mgrp { group });
        }
        if opts.mos {
            unencrypted.push(Extension::Mos);
        }
        let mut encrypted: Vec<Extension> = Vec::new();
        if let Some(group) = opts.send_mpan {
            let (_, state) = self.groups.get(&group).ok_or(Error::Malformed("unknown multicast group"))?;
            encrypted.push(Extension::Mpan { group, inner_state: *state });
        }
        let seq = self.next_sequence(peer)?;
        self.seal(peer, seq, &unencrypted, &encrypted, command, &keys.key_ccm, &nonce)
    }

    #[allow(clippy::too_many_arguments)]
    fn seal(
        &self,
        dest: u16,
        seq: u8,
        unencrypted: &[Extension],
        encrypted: &[Extension],
        command: &[u8],
        key: &[u8; 16],
        nonce: &[u8; 13],
    ) -> Result<Vec<u8>, Error> {
        let mut header = vec![seq, 0];
        if !unencrypted.is_empty() {
            header[1] |= 0x01;
            encode_extensions(&unencrypted.iter().collect::<Vec<_>>(), &mut header);
        }
        let mut plain = Vec::with_capacity(command.len() + 20);
        if !encrypted.is_empty() {
            header[1] |= 0x02;
            encode_extensions(&encrypted.iter().collect::<Vec<_>>(), &mut plain);
        }
        plain.extend_from_slice(command);
        let total = 2 + header.len() + plain.len() + CCM_TAG_LEN;
        let a = aad(self.own_node, dest, self.home_id, total, &header);
        let ct = ccm_encrypt(key, nonce, &a, &plain);
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&[CC_SECURITY_2, cmd::MESSAGE_ENCAPSULATION]);
        out.extend_from_slice(&header);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    fn candidates(&self, peer: u16) -> Vec<KeyId> {
        let mut c = Vec::new();
        if self.temp.contains_key(&peer) {
            c.push(KeyId::Temporary);
        }
        match self.peer_classes.get(&peer) {
            Some(classes) if !classes.is_empty() => c.extend(
                classes
                    .iter()
                    .filter(|k| self.keys.contains_key(k))
                    .map(|k| KeyId::Class(*k)),
            ),
            // A peer whose classes are unknown (a network taken over): try every key.
            _ => c.extend(self.keys.keys().rev().map(|k| KeyId::Class(*k))),
        }
        c
    }

    fn open(&self, frame: &[u8], enc: &Encapsulation, sender: u16, dest: u16, key: &[u8; 16], nonce: &[u8; 13]) -> Option<Vec<u8>> {
        let a = aad(sender, dest, self.home_id, frame.len(), &frame[2..2 + enc.header_len]);
        ccm_decrypt(key, nonce, &a, &frame[2 + enc.header_len..])
    }

    /// Decrypts a singlecast Message Encapsulation from `peer` (`frame` starts at `0x9F 0x03`).
    ///
    /// On [`Error::DecryptFailed`] or [`Error::NoSpan`] the caller answers
    /// with [`nonce_report`](Self::nonce_report) so the peer resynchronizes.
    pub fn decrypt(&mut self, peer: u16, frame: &[u8]) -> Result<Decrypted, Error> {
        let enc = Encapsulation::parse(frame)?;
        if self.rx_seq.get(&peer) == Some(&enc.sequence) {
            return Err(Error::Duplicate);
        }
        let own = self.own_node;
        let (key, plain) = match (self.spans.remove(&peer), enc.sender_ei()) {
            (Some(Span::LocalEi(rei)), Some(sei)) => {
                let mut found = None;
                for key in self.candidates(peer) {
                    let keys = self.keys_for(peer, key)?;
                    let mut drbg = CtrDrbg::new(&mei(&nonce_prk(&sei, &rei)), &keys.personalization);
                    let nonce = drbg.next_nonce();
                    if let Some(p) = self.open(frame, &enc, peer, own, &keys.key_ccm, &nonce) {
                        found = Some((key, drbg, p));
                        break;
                    }
                }
                let (key, drbg, p) = found.ok_or(Error::DecryptFailed)?;
                self.spans.insert(peer, Span::Established { key, drbg });
                (key, p)
            }
            (Some(Span::Established { key, mut drbg }), None) => {
                let ccm = self.keys_for(peer, key)?.key_ccm;
                let mut plain = None;
                for _ in 0..MAX_DECRYPT_ATTEMPTS {
                    let nonce = drbg.next_nonce();
                    if let Some(p) = self.open(frame, &enc, peer, own, &ccm, &nonce) {
                        plain = Some(p);
                        break;
                    }
                }
                let p = plain.ok_or(Error::DecryptFailed)?;
                self.spans.insert(peer, Span::Established { key, drbg });
                (key, p)
            }
            (Some(s @ Span::LocalEi(_)), None) => {
                // A frame from before our Nonce Report; keep waiting for the SPAN.
                self.spans.insert(peer, s);
                return Err(Error::NoSpan);
            }
            (Some(Span::RemoteEi(rei)), _) => {
                // We hold the peer's REI for our next frame; its frame cannot be ours to decrypt.
                self.spans.insert(peer, Span::RemoteEi(rei));
                return Err(Error::NoSpan);
            }
            _ => return Err(Error::NoSpan),
        };
        self.rx_seq.insert(peer, enc.sequence);
        let mut extensions = enc.extensions.clone();
        let command = if enc.encrypted_extensions {
            let (exts, used) = parse_extensions(&plain)?;
            for e in &exts {
                if let (Extension::Mpan { group, inner_state }, true) = (e, e.is_encrypted()) {
                    self.peer_mpans.insert((peer, *group), *inner_state);
                }
            }
            extensions.extend(exts);
            plain[used..].to_vec()
        } else {
            plain
        };
        if command.is_empty() {
            return Err(Error::Malformed("empty encapsulated command"));
        }
        Ok(Decrypted {
            command,
            key,
            sequence: enc.sequence,
            extensions,
        })
    }

    /// Creates (or rotates) an own multicast group protected with `class`.
    pub fn create_group(&mut self, group: u8, class: SecurityClass) -> Result<(), Error> {
        if !self.keys.contains_key(&class) {
            return Err(Error::NoKey(class));
        }
        self.groups.insert(group, (class, random_bytes::<16>()?));
        Ok(())
    }

    /// Encrypts a multicast frame for an own group (MGRP extension, nonce from the MPAN).
    pub fn encrypt_multicast(&mut self, group: u8, command: &[u8]) -> Result<Vec<u8>, Error> {
        let (class, state) = self.groups.get_mut(&group).ok_or(Error::Malformed("unknown multicast group"))?;
        let class = *class;
        let keys = self.keys.get(&class).ok_or(Error::NoKey(class))?;
        let mut nonce = [0u8; 13];
        nonce.copy_from_slice(&aes128_ecb(&keys.key_mpan, state)[..13]);
        increment(state);
        let key = keys.key_ccm;
        // Multicast frames carry no sequence state per peer; use group 0x100+ as the counter key.
        let seq = self.next_sequence(0xff00 | u16::from(group))?;
        self.seal(u16::from(group), seq, &[Extension::Mgrp { group }], &[], command, &key, &nonce)
    }

    /// Decrypts a multicast frame from `peer` for `group` using the MPAN the peer gave us.
    ///
    /// On [`Error::NoSpan`] (no MPAN known) or [`Error::DecryptFailed`], the
    /// caller answers with [`nonce_report_mos`](Self::nonce_report_mos).
    pub fn decrypt_multicast(&mut self, peer: u16, frame: &[u8]) -> Result<Decrypted, Error> {
        let enc = Encapsulation::parse(frame)?;
        let group = enc.multicast_group().ok_or(Error::Malformed("multicast without MGRP"))?;
        let mut state = *self.peer_mpans.get(&(peer, group)).ok_or(Error::NoSpan)?;
        let class = self.highest_class(peer).ok_or(Error::NoSpan)?;
        let keys = self.keys.get(&class).ok_or(Error::NoKey(class))?;
        let (mpan_key, ccm) = (keys.key_mpan, keys.key_ccm);
        for _ in 0..MAX_DECRYPT_ATTEMPTS {
            let mut nonce = [0u8; 13];
            nonce.copy_from_slice(&aes128_ecb(&mpan_key, &state)[..13]);
            increment(&mut state);
            if let Some(p) = self.open(frame, &enc, peer, u16::from(group), &ccm, &nonce) {
                self.peer_mpans.insert((peer, group), state);
                return Ok(Decrypted {
                    command: p,
                    key: KeyId::Class(class),
                    sequence: enc.sequence,
                    extensions: enc.extensions,
                });
            }
        }
        self.peer_mpans.remove(&(peer, group));
        Err(Error::DecryptFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kex::{bootstrap_keys, KeyPair};

    const HOME: u32 = 0xc0ff_ee01;

    fn pair() -> (S2Manager, S2Manager) {
        let mut ctrl = S2Manager::new(1, HOME);
        let mut node = S2Manager::new(5, HOME);
        let key = [0x42u8; 16];
        ctrl.set_network_key(SecurityClass::S2Authenticated, &key);
        node.set_network_key(SecurityClass::S2Authenticated, &key);
        ctrl.set_peer_classes(5, &[SecurityClass::S2Authenticated]);
        node.set_peer_classes(1, &[SecurityClass::S2Authenticated]);
        (ctrl, node)
    }

    /// Runs the Nonce Get / Nonce Report exchange from `a` (sender) to `b`.
    fn sync(a: &mut S2Manager, a_id: u16, b: &mut S2Manager, b_id: u16) {
        let _get = a.nonce_get(b_id).unwrap();
        let report = b.nonce_report(a_id, false).unwrap();
        assert!(a.handle_nonce_report(b_id, &report).unwrap().sos);
    }

    #[test]
    fn span_roundtrip_both_directions() {
        let (mut c, mut n) = pair();
        assert_eq!(c.encrypt(5, &[0x31, 0x04], &EncryptOptions::default()), Err(Error::NoSpan));
        sync(&mut c, 1, &mut n, 5);
        let f1 = c.encrypt(5, &[0x31, 0x04], &EncryptOptions::default()).unwrap();
        assert!(Encapsulation::parse(&f1).unwrap().sender_ei().is_some());
        let d = n.decrypt(1, &f1).unwrap();
        assert_eq!(d.command, vec![0x31, 0x04]);
        assert_eq!(d.key, KeyId::Class(SecurityClass::S2Authenticated));
        // Same SPAN, no extension, both directions draw from one generator.
        let f2 = n.encrypt(1, &[0x31, 0x05, 0x01, 0x22, 0x00, 0xd2], &EncryptOptions::default()).unwrap();
        assert!(Encapsulation::parse(&f2).unwrap().extensions.is_empty());
        assert_eq!(c.decrypt(5, &f2).unwrap().command, vec![0x31, 0x05, 0x01, 0x22, 0x00, 0xd2]);
        let f3 = c.encrypt(5, &[0x25, 0x01, 0xff], &EncryptOptions::default()).unwrap();
        assert_eq!(n.decrypt(1, &f3).unwrap().command, vec![0x25, 0x01, 0xff]);
        // A retransmission is a duplicate.
        assert_eq!(n.decrypt(1, &f3), Err(Error::Duplicate));
    }

    #[test]
    fn receiver_skips_lost_frames() {
        let (mut c, mut n) = pair();
        sync(&mut c, 1, &mut n, 5);
        let f1 = c.encrypt(5, &[0x20, 0x02], &EncryptOptions::default()).unwrap();
        n.decrypt(1, &f1).unwrap();
        let _lost = c.encrypt(5, &[0x20, 0x02], &EncryptOptions::default()).unwrap();
        let f3 = c.encrypt(5, &[0x20, 0x01, 0x10], &EncryptOptions::default()).unwrap();
        assert_eq!(n.decrypt(1, &f3).unwrap().command, vec![0x20, 0x01, 0x10]);
    }

    #[test]
    fn tampering_and_wrong_key_fail() {
        let (mut c, mut n) = pair();
        sync(&mut c, 1, &mut n, 5);
        let mut f = c.encrypt(5, &[0x20, 0x02], &EncryptOptions::default()).unwrap();
        let last = f.len() - 1;
        f[last] ^= 0x01;
        assert_eq!(n.decrypt(1, &f), Err(Error::DecryptFailed));
        // After a failure the receiver answers with a fresh REI and the sender resyncs.
        let report = n.nonce_report(1, false).unwrap();
        c.handle_nonce_report(5, &report).unwrap();
        let f = c.encrypt(5, &[0x20, 0x02], &EncryptOptions::default()).unwrap();
        assert_eq!(n.decrypt(1, &f).unwrap().command, vec![0x20, 0x02]);

        let mut other = S2Manager::new(5, HOME);
        other.set_network_key(SecurityClass::S2Authenticated, &[0x43; 16]);
        other.set_peer_classes(1, &[SecurityClass::S2Authenticated]);
        sync(&mut c, 1, &mut other, 5);
        let f = c.encrypt(5, &[0x20, 0x02], &EncryptOptions::default()).unwrap();
        assert_eq!(other.decrypt(1, &f), Err(Error::DecryptFailed));
    }

    #[test]
    fn home_id_and_node_ids_are_authenticated() {
        let (mut c, _) = pair();
        let mut n = S2Manager::new(5, HOME ^ 1);
        n.set_network_key(SecurityClass::S2Authenticated, &[0x42; 16]);
        n.set_peer_classes(1, &[SecurityClass::S2Authenticated]);
        sync(&mut c, 1, &mut n, 5);
        let f = c.encrypt(5, &[0x20, 0x02], &EncryptOptions::default()).unwrap();
        assert_eq!(n.decrypt(1, &f), Err(Error::DecryptFailed));
    }

    #[test]
    fn unknown_peer_class_tries_every_key() {
        let (mut c, _) = pair();
        let mut n = S2Manager::new(5, HOME);
        n.set_network_key(SecurityClass::S2Unauthenticated, &[0x11; 16]);
        n.set_network_key(SecurityClass::S2Authenticated, &[0x42; 16]);
        sync(&mut c, 1, &mut n, 5);
        let f = c.encrypt(5, &[0x20, 0x02], &EncryptOptions::default()).unwrap();
        assert_eq!(n.decrypt(1, &f).unwrap().key, KeyId::Class(SecurityClass::S2Authenticated));
    }

    #[test]
    fn bootstrap_with_temporary_key() {
        let ckp = KeyPair::generate().unwrap();
        let nkp = KeyPair::generate().unwrap();
        let mut c = S2Manager::new(1, HOME);
        let mut n = S2Manager::new(5, HOME);
        c.begin_bootstrap(5, bootstrap_keys(&ckp.shared_secret(nkp.public()).unwrap(), ckp.public(), nkp.public()));
        n.begin_bootstrap(1, bootstrap_keys(&nkp.shared_secret(ckp.public()).unwrap(), ckp.public(), nkp.public()));
        sync(&mut n, 5, &mut c, 1);
        let f = n.encrypt(1, &[0x9f, 0x06, 0x01, 0x02, 0x01, 0x02], &EncryptOptions::default()).unwrap();
        let d = c.decrypt(5, &f).unwrap();
        assert_eq!(d.key, KeyId::Temporary);
        assert_eq!(d.command, vec![0x9f, 0x06, 0x01, 0x02, 0x01, 0x02]);
    }

    #[test]
    fn mpan_multicast() {
        let (mut c, mut n) = pair();
        c.create_group(3, SecurityClass::S2Authenticated).unwrap();
        // The node lacks the MPAN.
        let m = c.encrypt_multicast(3, &[0x25, 0x01, 0xff]).unwrap();
        assert_eq!(n.decrypt_multicast(1, &m), Err(Error::NoSpan));
        // The follow-up singlecast hands it over in an encrypted extension.
        sync(&mut c, 1, &mut n, 5);
        let opts = EncryptOptions { follow_up_group: Some(3), send_mpan: Some(3), ..Default::default() };
        let fu = c.encrypt(5, &[0x25, 0x01, 0xff], &opts).unwrap();
        let d = n.decrypt(1, &fu).unwrap();
        assert_eq!(d.command, vec![0x25, 0x01, 0xff]);
        assert!(d.extensions.iter().any(|e| matches!(e, Extension::Mpan { group: 3, .. })));
        let m2 = c.encrypt_multicast(3, &[0x25, 0x01, 0x00]).unwrap();
        assert_eq!(n.decrypt_multicast(1, &m2).unwrap().command, vec![0x25, 0x01, 0x00]);
    }

    #[test]
    fn extension_parsing_is_strict() {
        assert!(parse_extensions(&[]).is_err());
        assert!(parse_extensions(&[1, 0x41]).is_err());
        assert!(parse_extensions(&[18, 0x41, 0]).is_err());
        assert_eq!(parse_extensions(&[2, 0x7f]), Err(Error::UnsupportedExtension(0x3f)));
        let (e, used) = parse_extensions(&[3, 0x3e, 9, 0xaa]).unwrap();
        assert_eq!(used, 3);
        assert_eq!(e, vec![Extension::Unknown { kind: 0x3e, data: vec![9] }]);
        assert!(Encapsulation::parse(&[0x9f, 0x03, 0, 0x01, 2, 0x84]).is_err());
    }
}
