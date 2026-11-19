//! The node store: one JSON file, written atomically and only when
//! something changed (batched over a short delay, so a burst of interview
//! results is one write, and reports never cause a write at all).

use crate::node::NodeRecord;
use crate::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use zwave_security::provisioning::ProvisioningList;

/// Format version of the file.
pub const VERSION: u32 = 1;

/// What is persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreData {
    /// Format version.
    pub version: u32,
    /// The network's home id, to refuse a store from another network.
    pub home_id: Option<u32>,
    /// The nodes.
    pub nodes: BTreeMap<NodeId, NodeRecord>,
    /// The SmartStart provisioning list.
    #[serde(default)]
    pub provisioning: ProvisioningList,
}

impl Default for StoreData {
    fn default() -> Self {
        StoreData {
            version: VERSION,
            home_id: None,
            nodes: BTreeMap::new(),
            provisioning: ProvisioningList::default(),
        }
    }
}

/// Reads a store; a missing file is an empty store.
pub fn load(path: &Path) -> Result<StoreData, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let d: StoreData =
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            if d.version > VERSION {
                return Err(format!(
                    "{}: store version {} is newer than this build",
                    path.display(),
                    d.version
                ));
            }
            Ok(d)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreData::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes a store atomically: a temporary file in the same directory,
/// synced, then renamed over the old one.
pub fn save(path: &Path, data: &StoreData) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?;
    let tmp: PathBuf = path.with_extension("json.tmp");
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(&json).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_missing() {
        let dir = std::env::temp_dir().join(format!("zwave-store-{}", std::process::id()));
        let path = dir.join("nodes.json");
        assert_eq!(load(&path).unwrap(), StoreData::default());
        let mut d = StoreData {
            home_id: Some(0xc0ffee01),
            ..Default::default()
        };
        d.nodes.insert(5, NodeRecord::new(5));
        save(&path, &d).unwrap();
        assert_eq!(load(&path).unwrap(), d);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
