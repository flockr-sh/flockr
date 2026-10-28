//! Flock files under `state_dir()/flock/`. Log entries are one file each,
//! named by hash, so the CLI and the server can both add entries without a
//! lock: a write is an atomic create, and reading is a directory listing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::cert::Cert;
use super::log::{self, Entry, State};
use crate::mobile::store::{atomic_create, atomic_write, ensure_dir};

pub(crate) const GROUP_FILE: &str = "group.json";
pub(crate) const LOG_DIR: &str = "log";
pub(crate) const CERT_FILE: &str = "cert.json";
pub(crate) const NODE_FILE: &str = "node.json";

pub(crate) fn flock_dir() -> PathBuf {
    crate::config::state_dir().join("flock")
}

#[derive(Serialize, Deserialize)]
struct GroupFile {
    version: u32,
    group: String,
}

pub(crate) fn load_group(dir: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(dir.join(GROUP_FILE)) {
        Ok(text) => {
            let file: GroupFile = serde_json::from_str(&text)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            Ok(Some(file.group))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Records which group this machine belongs to. Fails if it already belongs
/// to one.
pub(crate) fn save_group(dir: &Path, group: &str) -> io::Result<()> {
    let bytes = serde_json::to_vec(&GroupFile {
        version: 1,
        group: group.to_owned(),
    })
    .map_err(io::Error::other)?;
    if atomic_create(&dir.join(GROUP_FILE), &bytes)? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "this machine already belongs to a flock",
        ))
    }
}

/// Saves `entry` unless it is already stored. `true` when it was new.
pub(crate) fn save_entry(dir: &Path, entry: &Entry) -> io::Result<bool> {
    let log_dir = dir.join(LOG_DIR);
    ensure_dir(&log_dir)?;
    let bytes = serde_json::to_vec(entry).map_err(io::Error::other)?;
    atomic_create(&log_dir.join(format!("{}.json", entry.hash())), &bytes)
}

pub(crate) fn load_entries(dir: &Path) -> io::Result<Vec<Entry>> {
    let read = match fs::read_dir(dir.join(LOG_DIR)) {
        Ok(read) => read,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut entries = Vec::new();
    for item in read {
        let path = item?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(entry) = serde_json::from_str::<Entry>(&text) {
            entries.push(entry);
        }
        if entries.len() > log::MAX_ENTRIES {
            break;
        }
    }
    Ok(entries)
}

/// The group state, or `None` when this machine is in no group.
pub(crate) fn load_state(dir: &Path) -> io::Result<Option<State>> {
    let Some(group) = load_group(dir)? else {
        return Ok(None);
    };
    let entries = load_entries(dir)?;
    log::fold(&entries, Some(&group))
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
}

/// Changes when an entry is added; cheap enough to poll.
pub(crate) fn log_stamp(dir: &Path) -> Option<SystemTime> {
    fs::metadata(dir.join(LOG_DIR))
        .and_then(|meta| meta.modified())
        .ok()
}

pub(crate) fn load_cert(dir: &Path) -> Option<Cert> {
    let text = fs::read_to_string(dir.join(CERT_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn save_cert(dir: &Path, cert: &Cert) -> io::Result<()> {
    let bytes = serde_json::to_vec(cert).map_err(io::Error::other)?;
    atomic_write(&dir.join(CERT_FILE), &bytes)
}

/// The running server's flock node, for `flockr flock status`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct NodeRecord {
    pub version: u32,
    pub pid: u32,
    pub bound: Vec<String>,
    /// Member IDs with a live, authenticated connection.
    #[serde(default)]
    pub connected: Vec<String>,
    pub updated_at: u64,
}

pub(crate) fn load_node(dir: &Path) -> Option<NodeRecord> {
    let text = fs::read_to_string(dir.join(NODE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn save_node(dir: &Path, record: &NodeRecord) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    atomic_write(&dir.join(NODE_FILE), &bytes)
}

pub(crate) fn remove_node_if_owned(dir: &Path, pid: u32) {
    if load_node(dir).is_some_and(|record| record.pid == pid) {
        let _ = fs::remove_file(dir.join(NODE_FILE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::log::genesis;
    use crate::flock::log::tests::owner;

    #[test]
    fn entries_round_trip_and_group_is_set_once() {
        let dir = crate::mobile::store::tests::temp_dir("flock-store");
        assert!(load_state(&dir).unwrap().is_none());
        let owner = owner();
        let root = genesis("home", &owner, 0);
        assert!(save_entry(&dir, &root).unwrap());
        assert!(!save_entry(&dir, &root).unwrap());
        save_group(&dir, &root.hash()).unwrap();
        assert!(save_group(&dir, "other").is_err());
        let state = load_state(&dir).unwrap().unwrap();
        assert_eq!(state.group, root.hash());
        assert!(log_stamp(&dir).is_some());
    }
}
