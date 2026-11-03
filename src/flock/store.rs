//! Flock files under `state_dir()/flock/`.
//!
//! - `hub.json`, on the hub only: the flock and its members. Only the CLI
//!   writes it; the hub's server re-reads it when it changes.
//! - `group.json`, on every other member: the flock it joined and how to
//!   reach the hub. Written once by `flockr flock join`.
//! - `members.json`, on every other member: the member list as the hub last
//!   sent it. Only the server writes it.
//! - `node.json`: the running server's flock state, for `flockr flock status`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mobile::store::{atomic_create, atomic_write};

pub(crate) const HUB_FILE: &str = "hub.json";
pub(crate) const GROUP_FILE: &str = "group.json";
pub(crate) const MEMBERS_FILE: &str = "members.json";
pub(crate) const NODE_FILE: &str = "node.json";
pub(crate) const MAX_NAME_CHARS: usize = 64;
pub(crate) const MEMBER_ID_PREFIX: &str = "m_";
pub(crate) const GROUP_ID_PREFIX: &str = "g_";

pub(crate) fn flock_dir() -> PathBuf {
    crate::config::state_dir().join("flock")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Admin,
    Member,
    Viewer,
}

impl Role {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Computer,
    Phone,
    Browser,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Computer => "computer",
            Self::Phone => "phone",
            Self::Browser => "browser",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Member {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub role: Role,
    /// X25519 Noise public key, base64url.
    pub noise_key: String,
    pub joined_at: u64,
}

/// The hub's record of the flock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HubFile {
    pub version: u32,
    pub group: String,
    pub name: String,
    /// The hub's own member ID; the hub is listed in `members`.
    pub hub: String,
    pub members: Vec<Member>,
}

impl HubFile {
    pub(crate) fn member(&self, id: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.id == id)
    }

    pub(crate) fn by_noise_key(&self, key: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.noise_key == key)
    }
}

/// A member's record of the flock it joined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GroupFile {
    pub version: u32,
    pub group: String,
    pub name: String,
    /// This machine's member ID.
    pub member: String,
    pub hub: String,
    /// The hub's Noise public key, pinned at join.
    pub hub_key: String,
    /// `host:port` addresses to dial the hub at.
    pub hub_addrs: Vec<String>,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn already_in_flock() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "this machine is already in a flock",
    )
}

pub(crate) fn load_hub(dir: &Path) -> io::Result<Option<HubFile>> {
    read_json(&dir.join(HUB_FILE))
}

/// Starts a flock with this machine as the hub. Fails if this machine is
/// already in one.
pub(crate) fn create_hub(dir: &Path, hub: &HubFile) -> io::Result<()> {
    if load_group(dir)?.is_some() {
        return Err(already_in_flock());
    }
    let bytes = serde_json::to_vec_pretty(hub).map_err(io::Error::other)?;
    if atomic_create(&dir.join(HUB_FILE), &bytes)? {
        Ok(())
    } else {
        Err(already_in_flock())
    }
}

/// Re-reads `hub.json`, applies `change`, and writes it back.
pub(crate) fn update_hub<T>(dir: &Path, change: impl FnOnce(&mut HubFile) -> T) -> io::Result<T> {
    let mut hub = load_hub(dir)?.ok_or_else(|| io::Error::other("this machine is not a hub"))?;
    let result = change(&mut hub);
    let bytes = serde_json::to_vec_pretty(&hub).map_err(io::Error::other)?;
    atomic_write(&dir.join(HUB_FILE), &bytes)?;
    Ok(result)
}

pub(crate) fn load_group(dir: &Path) -> io::Result<Option<GroupFile>> {
    read_json(&dir.join(GROUP_FILE))
}

/// Records the flock this machine joined. Fails if it is already in one.
pub(crate) fn save_group(dir: &Path, group: &GroupFile) -> io::Result<()> {
    if load_hub(dir)?.is_some() {
        return Err(already_in_flock());
    }
    let bytes = serde_json::to_vec_pretty(group).map_err(io::Error::other)?;
    if atomic_create(&dir.join(GROUP_FILE), &bytes)? {
        Ok(())
    } else {
        Err(already_in_flock())
    }
}

/// The member list as the hub last sent it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MembersCache {
    pub version: u32,
    pub members: Vec<Member>,
    /// Members connected to the hub, the hub included.
    pub connected: Vec<String>,
    pub updated_at: u64,
}

pub(crate) fn load_members_cache(dir: &Path) -> Option<MembersCache> {
    read_json(&dir.join(MEMBERS_FILE)).ok().flatten()
}

pub(crate) fn save_members_cache(dir: &Path, cache: &MembersCache) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(cache).map_err(io::Error::other)?;
    atomic_write(&dir.join(MEMBERS_FILE), &bytes)
}

/// The running server's flock node, for `flockr flock status`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct NodeRecord {
    pub version: u32,
    pub pid: u32,
    /// Addresses the hub listens on; empty on other members.
    pub bound: Vec<String>,
    /// On the hub, connected member IDs; elsewhere, the hub's ID while
    /// connected to it.
    #[serde(default)]
    pub connected: Vec<String>,
    /// Why the hub last refused this member, if it did.
    #[serde(default)]
    pub refused: Option<String>,
    pub updated_at: u64,
}

impl NodeRecord {
    /// Equal apart from `updated_at`.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        Self {
            updated_at: other.updated_at,
            ..self.clone()
        } == *other
    }
}

pub(crate) fn load_node(dir: &Path) -> Option<NodeRecord> {
    read_json(&dir.join(NODE_FILE)).ok().flatten()
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
pub(crate) mod tests {
    use super::*;

    pub(crate) fn member(id: &str, noise_key: &str) -> Member {
        Member {
            id: id.to_owned(),
            name: id.to_owned(),
            kind: Kind::Computer,
            role: Role::Admin,
            noise_key: noise_key.to_owned(),
            joined_at: 0,
        }
    }

    fn hub() -> HubFile {
        HubFile {
            version: 1,
            group: "g_1".into(),
            name: "home".into(),
            hub: "m_hub".into(),
            members: vec![member("m_hub", "k_hub")],
        }
    }

    fn group() -> GroupFile {
        GroupFile {
            version: 1,
            group: "g_1".into(),
            name: "home".into(),
            member: "m_a".into(),
            hub: "m_hub".into(),
            hub_key: "k_hub".into(),
            hub_addrs: vec!["10.0.0.1:47392".into()],
        }
    }

    #[test]
    fn a_machine_is_a_hub_or_a_member_once() {
        let dir = crate::mobile::store::tests::temp_dir("flock-store-hub");
        assert!(load_hub(&dir).unwrap().is_none());
        create_hub(&dir, &hub()).unwrap();
        assert!(create_hub(&dir, &hub()).is_err());
        assert!(save_group(&dir, &group()).is_err());
        update_hub(&dir, |hub| hub.members.push(member("m_a", "k_a"))).unwrap();
        let loaded = load_hub(&dir).unwrap().unwrap();
        assert_eq!(loaded.by_noise_key("k_a").unwrap().id, "m_a");
        assert!(loaded.member("m_b").is_none());

        let dir = crate::mobile::store::tests::temp_dir("flock-store-member");
        save_group(&dir, &group()).unwrap();
        assert!(save_group(&dir, &group()).is_err());
        assert!(create_hub(&dir, &hub()).is_err());
        assert!(update_hub(&dir, |_| ()).is_err());
        assert_eq!(load_group(&dir).unwrap().unwrap(), group());
    }
}
