//! The membership log: an append-only set of signed entries, each naming the
//! entries it follows. Folding the log gives the group's members and roles.
//!
//! Rules, applied in topological order with ties broken by entry hash:
//! - Each entry needs a valid signature, the group's ID and known parents.
//!   Entries with missing parents wait until the parents arrive.
//! - Only the owner key adds members, changes roles and replaces keys.
//! - The owner removes anyone; an admin removes members and viewers.
//! - A removed member stays removed; a later or concurrent add is ignored.
//! - When an admin is removed, its entries that are not ancestors of the
//!   removal are void.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::keys::{self, decode_key, encode_key};
use crate::mobile::store::base64url;

pub(crate) const MAX_ENTRIES: usize = 10_000;
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024;
pub(crate) const MAX_NAME_CHARS: usize = 64;
pub(crate) const MAX_ADDRS: usize = 8;
pub(crate) const MAX_ADDR_CHARS: usize = 64;
const SIGNING_CONTEXT: &[u8] = b"flockr.flock.entry.v1\0";
/// Bound on re-folds after admin removals void entries.
const MAX_VOID_PASSES: usize = 8;

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
pub(crate) struct MemberInfo {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Ed25519 public key, base64url.
    pub sign_key: String,
    /// X25519 Noise public key, base64url.
    pub noise_key: String,
    /// `host:port` addresses where the member's server listens.
    #[serde(default)]
    pub addrs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Op {
    Genesis {
        name: String,
        owner: String,
    },
    Add {
        member: MemberInfo,
        role: Role,
    },
    SetRole {
        member: String,
        role: Role,
    },
    Rotate {
        member: String,
        sign_key: String,
        noise_key: String,
    },
    Remove {
        member: String,
    },
    SetAddrs {
        member: String,
        addrs: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Body {
    /// Group ID; empty in the genesis entry, whose hash is the group ID.
    pub group: String,
    pub parents: Vec<String>,
    /// Signing public key, base64url.
    pub author: String,
    /// Unix seconds, for display only.
    pub at: u64,
    pub op: Op,
}

/// A signed entry. The body is kept as the exact signed JSON so that fields
/// added by newer versions survive the round trip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub body: String,
    pub sig: String,
}

fn signed_bytes(body: &str) -> Vec<u8> {
    let mut bytes = SIGNING_CONTEXT.to_vec();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

impl Entry {
    pub(crate) fn create(body: &Body, key: &SigningKey) -> Self {
        let body = serde_json::to_string(body).unwrap_or_default();
        let sig = keys::sign(key, &signed_bytes(&body));
        Self { body, sig }
    }

    pub(crate) fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(signed_bytes(&self.body));
        hasher.update(self.sig.as_bytes());
        base64url(&hasher.finalize())
    }

    /// The body, when it parses and its signature checks out.
    pub(crate) fn verified_body(&self) -> Option<Body> {
        if self.body.len() > MAX_BODY_BYTES {
            return None;
        }
        let body: Body = serde_json::from_str(&self.body).ok()?;
        keys::verify(&body.author, &signed_bytes(&self.body), &self.sig).then_some(body)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Member {
    pub info: MemberInfo,
    pub role: Role,
    pub removed: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    pub group: String,
    pub name: String,
    /// Owner public key, base64url.
    pub owner: String,
    pub members: BTreeMap<String, Member>,
    /// Entries no other entry names as a parent.
    pub heads: Vec<String>,
    /// Hashes of every entry folded in, valid or not.
    pub known: HashSet<String>,
    /// Entries that failed a rule, with the reason.
    pub rejected: Vec<(String, String)>,
    /// Entries waiting for missing parents.
    pub pending: usize,
}

impl State {
    pub(crate) fn active(&self, id: &str) -> Option<&Member> {
        self.members.get(id).filter(|member| !member.removed)
    }

    pub(crate) fn active_by_noise_key(&self, noise_key: &str) -> Option<&Member> {
        self.members
            .values()
            .find(|member| !member.removed && member.info.noise_key == noise_key)
    }

    fn active_by_sign_key(&self, sign_key: &str) -> Option<&Member> {
        self.members
            .values()
            .find(|member| !member.removed && member.info.sign_key == sign_key)
    }

    /// A new entry signed by `key` that follows every current head.
    pub(crate) fn append(&self, op: Op, key: &SigningKey, at: u64) -> Entry {
        let mut parents = self.heads.clone();
        parents.sort();
        Entry::create(
            &Body {
                group: self.group.clone(),
                parents,
                author: encode_key(&key.verifying_key().to_bytes()),
                at,
                op,
            },
            key,
        )
    }
}

/// The entry that starts a group. Its hash is the group ID.
pub(crate) fn genesis(name: &str, owner: &SigningKey, at: u64) -> Entry {
    let owner_key = encode_key(&owner.verifying_key().to_bytes());
    Entry::create(
        &Body {
            group: String::new(),
            parents: Vec::new(),
            author: owner_key.clone(),
            at,
            op: Op::Genesis {
                name: name.to_owned(),
                owner: owner_key,
            },
        },
        owner,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FoldError {
    NoGenesis,
    /// Several genesis entries and no group ID to pick one.
    AmbiguousGenesis,
    TooManyEntries,
}

impl std::fmt::Display for FoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoGenesis => "the log has no genesis entry for this group",
            Self::AmbiguousGenesis => "the log holds more than one group",
            Self::TooManyEntries => "the log is too large",
        })
    }
}

struct Node {
    body: Body,
}

/// An accepted removal of an admin: its entry and every key that admin had.
struct AdminRemoval {
    entry: String,
    keys: Vec<String>,
}

fn valid_member_info(info: &MemberInfo) -> Result<(), &'static str> {
    let sign = decode_key(&info.sign_key).ok_or("member signing key is malformed")?;
    decode_key(&info.noise_key).ok_or("member noise key is malformed")?;
    if keys::member_id(&sign) != info.id {
        return Err("member ID does not match its signing key");
    }
    if info.name.trim().is_empty() || info.name.chars().count() > MAX_NAME_CHARS {
        return Err("member name is empty or too long");
    }
    valid_addrs(&info.addrs)
}

fn valid_addrs(addrs: &[String]) -> Result<(), &'static str> {
    if addrs.len() > MAX_ADDRS || addrs.iter().any(|addr| addr.len() > MAX_ADDR_CHARS) {
        return Err("too many or too long addresses");
    }
    Ok(())
}

/// Applies `op` to `state` if `author` may. `removal` is set when an admin
/// was removed.
fn apply(
    state: &mut State,
    key_history: &mut HashMap<String, Vec<String>>,
    hash: &str,
    body: &Body,
    removal: &mut Option<AdminRemoval>,
) -> Result<(), &'static str> {
    let by_owner = body.author == state.owner;
    match &body.op {
        Op::Genesis { .. } => Err("only the first entry can be a genesis"),
        Op::Add { member, role } => {
            if !by_owner {
                return Err("only the owner can add members");
            }
            valid_member_info(member)?;
            match state.members.get(&member.id) {
                Some(existing) if existing.removed => {
                    return Err("a removed member can't be added again")
                }
                Some(_) => return Err("already a member"),
                None => {}
            }
            if state.active_by_noise_key(&member.noise_key).is_some()
                || state.active_by_sign_key(&member.sign_key).is_some()
            {
                return Err("another member uses this key");
            }
            key_history
                .entry(member.id.clone())
                .or_default()
                .push(member.sign_key.clone());
            state.members.insert(
                member.id.clone(),
                Member {
                    info: member.clone(),
                    role: *role,
                    removed: false,
                },
            );
            Ok(())
        }
        Op::SetRole { member, role } => {
            if !by_owner {
                return Err("only the owner can change roles");
            }
            let target = state
                .members
                .get_mut(member)
                .filter(|target| !target.removed)
                .ok_or("no such member")?;
            target.role = *role;
            Ok(())
        }
        Op::Rotate {
            member,
            sign_key,
            noise_key,
        } => {
            if !by_owner {
                return Err("only the owner can replace keys");
            }
            if decode_key(sign_key).is_none() || decode_key(noise_key).is_none() {
                return Err("replacement key is malformed");
            }
            if state.members.values().any(|other| {
                other.info.id != *member
                    && !other.removed
                    && (other.info.sign_key == *sign_key || other.info.noise_key == *noise_key)
            }) {
                return Err("another member uses this key");
            }
            let target = state
                .members
                .get_mut(member)
                .filter(|target| !target.removed)
                .ok_or("no such member")?;
            target.info.sign_key = sign_key.clone();
            target.info.noise_key = noise_key.clone();
            key_history
                .entry(member.clone())
                .or_default()
                .push(sign_key.clone());
            Ok(())
        }
        Op::Remove { member } => {
            let target = state.active(member).ok_or("no such member")?;
            let target_is_admin = target.role == Role::Admin;
            let by_admin = state
                .active_by_sign_key(&body.author)
                .is_some_and(|author| author.role == Role::Admin);
            let allowed = by_owner || (by_admin && !target_is_admin);
            if !allowed {
                return Err("only the owner removes admins; admins remove other members");
            }
            if let Some(target) = state.members.get_mut(member) {
                target.removed = true;
            }
            if target_is_admin {
                *removal = Some(AdminRemoval {
                    entry: hash.to_owned(),
                    keys: key_history.get(member).cloned().unwrap_or_default(),
                });
            }
            Ok(())
        }
        Op::SetAddrs { member, addrs } => {
            valid_addrs(addrs)?;
            let target = state
                .members
                .get_mut(member)
                .filter(|target| !target.removed)
                .ok_or("no such member")?;
            if !by_owner && target.info.sign_key != body.author {
                return Err("only the owner or the member itself sets addresses");
            }
            target.info.addrs = addrs.clone();
            Ok(())
        }
    }
}

fn ancestors(nodes: &HashMap<String, Node>, start: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![start.to_owned()];
    while let Some(hash) = stack.pop() {
        if let Some(node) = nodes.get(&hash) {
            for parent in &node.body.parents {
                if seen.insert(parent.clone()) {
                    stack.push(parent.clone());
                }
            }
        }
    }
    seen
}

/// A removal that fails against the running state can still be valid against
/// the state its author saw: a concurrent change, such as promoting the target
/// to admin, must not cancel it. Applies the removal when that state allows it.
#[allow(clippy::too_many_arguments)] // Fold internals; a struct would only rename them.
fn causal_remove(
    nodes: &HashMap<String, Node>,
    order: &[String],
    void: &HashSet<String>,
    hash: &str,
    body: &Body,
    state: &mut State,
    key_history: &HashMap<String, Vec<String>>,
    removal: &mut Option<AdminRemoval>,
) -> bool {
    let Op::Remove { member } = &body.op else {
        return false;
    };
    if state.active(member).is_none() {
        return false;
    }
    let before = ancestors(nodes, hash);
    let mut seen = State {
        group: state.group.clone(),
        name: state.name.clone(),
        owner: state.owner.clone(),
        ..State::default()
    };
    let mut seen_keys = HashMap::new();
    for earlier in order.iter().skip(1) {
        if before.contains(earlier) && !void.contains(earlier) {
            let _ = apply(
                &mut seen,
                &mut seen_keys,
                earlier,
                &nodes[earlier].body,
                &mut None,
            );
        }
    }
    let mut seen_removal = None;
    if apply(&mut seen, &mut seen_keys, hash, body, &mut seen_removal).is_err() {
        return false;
    }
    let target_is_admin = state
        .active(member)
        .is_some_and(|target| target.role == Role::Admin);
    if let Some(target) = state.members.get_mut(member) {
        target.removed = true;
    }
    if target_is_admin {
        *removal = Some(AdminRemoval {
            entry: hash.to_owned(),
            keys: key_history.get(member).cloned().unwrap_or_default(),
        });
    }
    true
}

/// Folds `entries` into the group state. `group` picks the genesis when the
/// set holds more than one.
pub(crate) fn fold(entries: &[Entry], group: Option<&str>) -> Result<State, FoldError> {
    if entries.len() > MAX_ENTRIES {
        return Err(FoldError::TooManyEntries);
    }
    let mut nodes: HashMap<String, Node> = HashMap::new();
    for entry in entries {
        if let Some(body) = entry.verified_body() {
            nodes.entry(entry.hash()).or_insert(Node { body });
        }
    }
    let geneses: Vec<&String> = nodes
        .iter()
        .filter(|(hash, node)| {
            matches!(&node.body.op, Op::Genesis { owner, .. } if *owner == node.body.author)
                && node.body.parents.is_empty()
                && node.body.group.is_empty()
                && group.is_none_or(|group| group == hash.as_str())
        })
        .map(|(hash, _)| hash)
        .collect();
    let root = match geneses.as_slice() {
        [] => return Err(FoldError::NoGenesis),
        [root] => (*root).clone(),
        _ => return Err(FoldError::AmbiguousGenesis),
    };
    let Op::Genesis { name, owner } = &nodes[&root].body.op else {
        return Err(FoldError::NoGenesis);
    };
    let (name, owner) = (name.clone(), owner.clone());

    // Kahn's algorithm over this group's entries; ties go to the lower hash.
    let mut waiting: HashMap<&str, usize> = HashMap::new();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (hash, node) in &nodes {
        if *hash == root || node.body.group != root || node.body.parents.is_empty() {
            continue;
        }
        let mut parents: Vec<&str> = node.body.parents.iter().map(String::as_str).collect();
        parents.sort_unstable();
        parents.dedup();
        waiting.insert(hash, parents.len());
        for parent in parents {
            children.entry(parent).or_default().push(hash);
        }
    }
    let mut order = Vec::new();
    let mut ready = BinaryHeap::from([Reverse(root.as_str())]);
    while let Some(Reverse(hash)) = ready.pop() {
        order.push(hash.to_owned());
        for child in children.get(hash).into_iter().flatten() {
            if let Some(count) = waiting.get_mut(child) {
                *count -= 1;
                if *count == 0 {
                    ready.push(Reverse(child));
                }
            }
        }
    }
    let pending = waiting.len() + 1 - order.len();

    let mut void: HashSet<String> = HashSet::new();
    let mut passes = 0;
    loop {
        let mut state = State {
            group: root.clone(),
            name: name.clone(),
            owner: owner.clone(),
            pending,
            ..State::default()
        };
        let mut key_history = HashMap::new();
        let mut removals = Vec::new();
        let mut accepted = Vec::new();
        for hash in order.iter().skip(1) {
            if void.contains(hash) {
                state
                    .rejected
                    .push((hash.clone(), "void: its admin was removed".into()));
                continue;
            }
            let mut removal = None;
            let body = &nodes[hash].body;
            match apply(&mut state, &mut key_history, hash, body, &mut removal) {
                Ok(()) => accepted.push(hash),
                Err(_)
                    if causal_remove(
                        &nodes,
                        &order,
                        &void,
                        hash,
                        body,
                        &mut state,
                        &key_history,
                        &mut removal,
                    ) =>
                {
                    accepted.push(hash)
                }
                Err(reason) => state.rejected.push((hash.clone(), reason.to_owned())),
            }
            removals.extend(removal);
        }
        let mut next_void = void.clone();
        for removal in &removals {
            let before = ancestors(&nodes, &removal.entry);
            for hash in &accepted {
                if **hash != removal.entry
                    && removal.keys.contains(&nodes[*hash].body.author)
                    && !before.contains(*hash)
                {
                    next_void.insert((*hash).clone());
                }
            }
        }
        passes += 1;
        if next_void == void || passes >= MAX_VOID_PASSES {
            let in_order: HashSet<&str> = order.iter().map(String::as_str).collect();
            let named: HashSet<&str> = order
                .iter()
                .flat_map(|hash| nodes[hash].body.parents.iter().map(String::as_str))
                .collect();
            let mut heads: Vec<String> = in_order
                .difference(&named)
                .map(|hash| (*hash).to_owned())
                .collect();
            heads.sort();
            state.heads = heads;
            state.known = order.into_iter().collect();
            return Ok(state);
        }
        void = next_void;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::flock::keys::MemberKeys;

    pub(crate) fn info(keys: &MemberKeys, name: &str) -> MemberInfo {
        MemberInfo {
            id: keys.member_id(),
            name: name.to_owned(),
            kind: Kind::Computer,
            sign_key: encode_key(&keys.sign_public()),
            noise_key: encode_key(&keys.noise_public().unwrap()),
            addrs: vec!["10.0.0.1:47392".into()],
        }
    }

    pub(crate) fn owner() -> SigningKey {
        MemberKeys::generate().unwrap().sign
    }

    fn add(log: &mut Vec<Entry>, owner: &SigningKey, keys: &MemberKeys, role: Role) -> Entry {
        let state = fold(log, None).unwrap();
        let entry = state.append(
            Op::Add {
                member: info(keys, "m"),
                role,
            },
            owner,
            1,
        );
        log.push(entry.clone());
        entry
    }

    #[test]
    fn owner_adds_and_removes_members() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let desktop = MemberKeys::generate().unwrap();
        let tower = MemberKeys::generate().unwrap();
        add(&mut log, &owner, &desktop, Role::Admin);
        add(&mut log, &owner, &tower, Role::Member);
        let state = fold(&log, None).unwrap();
        assert_eq!(state.group, log[0].hash());
        assert_eq!(state.name, "home");
        assert!(state.active(&desktop.member_id()).is_some());
        assert!(state.active(&tower.member_id()).is_some());
        assert_eq!(state.heads, vec![log[2].hash()]);
        assert!(state.rejected.is_empty());

        // An admin removes a member; the member can't be added back.
        log.push(state.append(
            Op::Remove {
                member: tower.member_id(),
            },
            &desktop.sign,
            2,
        ));
        add(&mut log, &owner, &tower, Role::Member);
        let state = fold(&log, None).unwrap();
        assert!(state.active(&tower.member_id()).is_none());
        assert_eq!(state.rejected.len(), 1);
        assert!(state.rejected[0].1.contains("removed"));
    }

    #[test]
    fn only_the_owner_adds_and_admins_cannot_remove_admins() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let a = MemberKeys::generate().unwrap();
        let b = MemberKeys::generate().unwrap();
        add(&mut log, &owner, &a, Role::Admin);
        add(&mut log, &owner, &b, Role::Admin);
        let state = fold(&log, None).unwrap();
        let stranger = MemberKeys::generate().unwrap();
        log.push(state.append(
            Op::Add {
                member: info(&stranger, "x"),
                role: Role::Admin,
            },
            &a.sign,
            2,
        ));
        let state = fold(&log, None).unwrap();
        log.push(state.append(
            Op::Remove {
                member: b.member_id(),
            },
            &a.sign,
            3,
        ));
        let state = fold(&log, None).unwrap();
        assert!(state.active(&stranger.member_id()).is_none());
        assert!(state.active(&b.member_id()).is_some());
        assert_eq!(state.rejected.len(), 2);
    }

    #[test]
    fn bad_signatures_other_groups_and_missing_parents_are_left_out() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let a = MemberKeys::generate().unwrap();
        let entry = add(&mut log, &owner, &a, Role::Member);

        let mut forged = entry.clone();
        forged.body = forged.body.replace("\"member\"", "\"admin\"");
        let state = fold(&[log[0].clone(), forged], None).unwrap();
        assert!(state.members.is_empty());

        let other = genesis("other", &owner, 0);
        let mut mixed = log.clone();
        mixed.push(other.clone());
        assert_eq!(fold(&mixed, None).unwrap_err(), FoldError::AmbiguousGenesis);
        let state = fold(&mixed, Some(&log[0].hash())).unwrap();
        assert!(state.active(&a.member_id()).is_some());

        let orphan = Entry::create(
            &Body {
                group: log[0].hash(),
                parents: vec!["missing".into()],
                author: encode_key(&owner.verifying_key().to_bytes()),
                at: 5,
                op: Op::Remove {
                    member: a.member_id(),
                },
            },
            &owner,
        );
        log.push(orphan);
        let state = fold(&log, None).unwrap();
        assert_eq!(state.pending, 1);
        assert!(state.active(&a.member_id()).is_some());
    }

    #[test]
    fn concurrent_removal_beats_add_in_either_order() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let admin = MemberKeys::generate().unwrap();
        let target = MemberKeys::generate().unwrap();
        add(&mut log, &owner, &admin, Role::Admin);
        add(&mut log, &owner, &target, Role::Member);
        let base = fold(&log, None).unwrap();
        // Two branches from the same heads: a removal and a role change.
        let remove = base.append(
            Op::Remove {
                member: target.member_id(),
            },
            &admin.sign,
            2,
        );
        let promote = base.append(
            Op::SetRole {
                member: target.member_id(),
                role: Role::Admin,
            },
            &owner,
            2,
        );
        for pair in [[remove.clone(), promote.clone()], [promote, remove]] {
            let mut both = log.clone();
            both.extend(pair);
            let state = fold(&both, None).unwrap();
            assert!(state.active(&target.member_id()).is_none());
            assert_eq!(state.heads.len(), 2);
        }
    }

    #[test]
    fn a_removed_admins_concurrent_entries_are_void() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let rogue = MemberKeys::generate().unwrap();
        let victim = MemberKeys::generate().unwrap();
        add(&mut log, &owner, &rogue, Role::Admin);
        add(&mut log, &owner, &victim, Role::Member);
        let base = fold(&log, None).unwrap();
        // The rogue admin removes a member while the owner removes the rogue.
        let rogue_entry = base.append(
            Op::Remove {
                member: victim.member_id(),
            },
            &rogue.sign,
            2,
        );
        let owner_entry = base.append(
            Op::Remove {
                member: rogue.member_id(),
            },
            &owner,
            2,
        );
        log.extend([rogue_entry, owner_entry]);
        let state = fold(&log, None).unwrap();
        assert!(state.active(&rogue.member_id()).is_none());
        assert!(state.active(&victim.member_id()).is_some());
        assert!(state
            .rejected
            .iter()
            .any(|(_, reason)| reason.starts_with("void")));
    }

    #[test]
    fn members_set_their_own_addresses_only() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let a = MemberKeys::generate().unwrap();
        let b = MemberKeys::generate().unwrap();
        add(&mut log, &owner, &a, Role::Member);
        add(&mut log, &owner, &b, Role::Member);
        let state = fold(&log, None).unwrap();
        log.push(state.append(
            Op::SetAddrs {
                member: a.member_id(),
                addrs: vec!["10.0.0.9:1".into()],
            },
            &a.sign,
            2,
        ));
        let state = fold(&log, None).unwrap();
        log.push(state.append(
            Op::SetAddrs {
                member: a.member_id(),
                addrs: vec!["evil:1".into()],
            },
            &b.sign,
            3,
        ));
        let state = fold(&log, None).unwrap();
        assert_eq!(
            state.active(&a.member_id()).unwrap().info.addrs,
            ["10.0.0.9:1"]
        );
        assert_eq!(state.rejected.len(), 1);
    }

    #[test]
    fn rotation_keeps_the_member_id_and_moves_the_keys() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let a = MemberKeys::generate().unwrap();
        add(&mut log, &owner, &a, Role::Admin);
        let fresh = MemberKeys::generate().unwrap();
        let state = fold(&log, None).unwrap();
        log.push(state.append(
            Op::Rotate {
                member: a.member_id(),
                sign_key: encode_key(&fresh.sign_public()),
                noise_key: encode_key(&fresh.noise_public().unwrap()),
            },
            &owner,
            2,
        ));
        let state = fold(&log, None).unwrap();
        let member = state.active(&a.member_id()).unwrap();
        assert_eq!(member.info.sign_key, encode_key(&fresh.sign_public()));
        assert_eq!(member.role, Role::Admin);
    }

    #[test]
    fn unknown_body_fields_survive_and_still_verify() {
        let owner = owner();
        let mut body: serde_json::Value =
            serde_json::from_str(&genesis("home", &owner, 0).body).unwrap();
        body["future"] = serde_json::json!(true);
        let body = serde_json::to_string(&body).unwrap();
        let entry = Entry {
            sig: keys::sign(&owner, &signed_bytes(&body)),
            body,
        };
        assert!(entry.verified_body().is_some());
        assert!(fold(&[entry], None).is_ok());
    }
}
