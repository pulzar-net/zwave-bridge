//! The SmartStart provisioning list a controller keeps.
//!
//! An entry is a node the user has scanned (usually from its QR code) before
//! powering it. When a SmartStart node asks for inclusion, its NWI home id is
//! looked up here; a match is included with the entry's granted keys and the
//! entry's DSK authenticates the key exchange without a PIN prompt.

use crate::dsk::Dsk;
use crate::qr::QrCode;
use crate::SecurityClass;

/// Whether an entry should be included when it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum EntryStatus {
    /// Include the node when it asks.
    #[default]
    Active,
    /// Keep the entry but ignore the node's requests.
    Inactive,
}

/// One provisioned node.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Entry {
    /// The node's DSK.
    pub dsk: Dsk,
    /// The keys to grant.
    pub granted: Vec<SecurityClass>,
    /// Active or inactive.
    #[cfg_attr(feature = "serde", serde(default))]
    pub status: EntryStatus,
    /// The node id once included.
    #[cfg_attr(feature = "serde", serde(default))]
    pub node_id: Option<u16>,
    /// A label for people.
    #[cfg_attr(feature = "serde", serde(default))]
    pub name: Option<String>,
}

impl Entry {
    /// An active entry from a scanned QR code, granting what the node requests.
    pub fn from_qr(qr: &QrCode) -> Self {
        Entry {
            dsk: qr.dsk,
            granted: qr.requested_classes.clone(),
            status: EntryStatus::Active,
            node_id: None,
            name: None,
        }
    }
}

/// The provisioning list. Small by nature (a building's worth of devices),
/// so a vector with linear lookups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProvisioningList {
    entries: Vec<Entry>,
}

/// Upper bound on entries, to keep a misbehaving client from growing the list without end.
pub const MAX_ENTRIES: usize = 1000;

impl ProvisioningList {
    /// All entries.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Adds or replaces the entry with the same DSK. Returns false when the list is full.
    pub fn upsert(&mut self, entry: Entry) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.dsk == entry.dsk) {
            *e = entry;
            return true;
        }
        if self.entries.len() >= MAX_ENTRIES {
            return false;
        }
        self.entries.push(entry);
        true
    }

    /// Removes the entry with this DSK.
    pub fn remove(&mut self, dsk: &Dsk) -> Option<Entry> {
        let i = self.entries.iter().position(|e| &e.dsk == dsk)?;
        Some(self.entries.remove(i))
    }

    /// The entry for a DSK.
    pub fn get(&self, dsk: &Dsk) -> Option<&Entry> {
        self.entries.iter().find(|e| &e.dsk == dsk)
    }

    /// Mutable entry for a DSK.
    pub fn get_mut(&mut self, dsk: &Dsk) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| &e.dsk == dsk)
    }

    /// The active entry whose NWI home id matches a SmartStart inclusion request.
    pub fn match_nwi_home_id(&self, nwi: &[u8; 4]) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.status == EntryStatus::Active && &e.dsk.nwi_home_id() == nwi)
    }

    /// Whether any entry is active (the controller then listens for SmartStart requests).
    pub fn has_active(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.status == EntryStatus::Active && e.node_id.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_by_nwi() {
        let mut l = ProvisioningList::default();
        let dsk: Dsk = "51525-35455-41654-24356-13311-14526-06640-31770"
            .parse()
            .unwrap();
        assert!(l.upsert(Entry {
            dsk,
            granted: vec![SecurityClass::S2Authenticated],
            status: EntryStatus::Active,
            node_id: None,
            name: None
        }));
        assert!(l.has_active());
        assert_eq!(l.match_nwi_home_id(&dsk.nwi_home_id()).unwrap().dsk, dsk);
        assert!(l.match_nwi_home_id(&[0, 0, 0, 0]).is_none());
        l.get_mut(&dsk).unwrap().status = EntryStatus::Inactive;
        assert!(l.match_nwi_home_id(&dsk.nwi_home_id()).is_none());
        assert!(l.remove(&dsk).is_some());
        assert!(l.entries().is_empty());
    }
}
