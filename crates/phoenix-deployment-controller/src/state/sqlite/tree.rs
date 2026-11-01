use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
};

use rusqlite::{params, Connection, OptionalExtension};

use super::{malformed, required, ControllerStoreError, Result, Word};
use crate::{
    identity::{
        BirthToken, CandidateDiscovery, DiscoveryId, ObservationFailure, ProcessId, ProcessIdentity,
    },
    ownership::{DiscoveryGap, OwnedIdentityStatus, OwnedProcessTree, UnresolvedDiscovery},
};

type IdentityOrdinals = BTreeMap<ProcessIdentity, i64>;

pub(super) fn ordinal(identities: &IdentityOrdinals, identity: ProcessIdentity) -> Result<i64> {
    required(
        identities.get(&identity).copied(),
        "unknown identity reference",
    )
}

pub(super) fn write(
    connection: &Connection,
    id: &[u8],
    root: Option<ProcessIdentity>,
    tree: Option<&OwnedProcessTree>,
) -> Result<IdentityOrdinals> {
    let mut identities = BTreeSet::new();
    identities.extend(root);
    if let Some(tree) = tree {
        identities.extend(tree.identities().keys().copied());
        for status in tree.identities().values() {
            if let OwnedIdentityStatus::Present { current_parent } = status {
                identities.extend(current_parent);
            }
        }
        for unresolved in tree.unresolved().values() {
            if let DiscoveryGap::DifferentProcess(identity) = unresolved.gap {
                identities.insert(identity);
            }
        }
    }
    let mut ordinals = BTreeMap::new();
    for (index, identity) in identities.into_iter().enumerate() {
        let ordinal = i64::try_from(index)
            .map_err(|_| ControllerStoreError::Malformed("identity ordinal overflow"))?;
        connection.execute(
            "INSERT INTO process_identities VALUES (?1,?2,?3,?4,?5)",
            params![
                id,
                ordinal,
                identity.pid().get(),
                Word(identity.birth().seconds()),
                identity.birth().nanos()
            ],
        )?;
        ordinals.insert(identity, ordinal);
    }
    if let Some(tree) = tree {
        write_tree(connection, id, tree, &ordinals)?;
    }
    Ok(ordinals)
}

fn write_tree(
    connection: &Connection,
    id: &[u8],
    tree: &OwnedProcessTree,
    ordinals: &IdentityOrdinals,
) -> Result<()> {
    connection.execute(
        "INSERT INTO ownership VALUES (?1,?2)",
        params![id, Word(tree.next_discovery())],
    )?;
    for (identity, status) in tree.identities() {
        let (tag, parent, failure) = match status {
            OwnedIdentityStatus::NeedsFreshObservation => ("fresh", None, None),
            OwnedIdentityStatus::Present { current_parent } => (
                "present",
                current_parent
                    .map(|parent| ordinal(ordinals, parent))
                    .transpose()?,
                None,
            ),
            OwnedIdentityStatus::VerifiedAbsent => ("absent", None, None),
            OwnedIdentityStatus::Uncertain(failure) => {
                ("uncertain", None, Some(failure_text(*failure)))
            }
        };
        connection.execute(
            "INSERT INTO owned_identities VALUES (?1,?2,?3,?4,?5)",
            params![id, ordinal(ordinals, *identity)?, tag, parent, failure],
        )?;
    }
    for parent in tree.pending_child_scans() {
        connection.execute(
            "INSERT INTO pending_child_scans VALUES (?1,?2)",
            params![id, ordinal(ordinals, *parent)?],
        )?;
    }
    for unresolved in tree.unresolved().values() {
        let discovery = unresolved.discovery;
        let (tag, failure, replacement) = match unresolved.gap {
            DiscoveryGap::NotYetObserved => ("not_observed", None, None),
            DiscoveryGap::Observation(failure) => {
                ("observation", Some(failure_text(failure)), None)
            }
            DiscoveryGap::DifferentProcess(identity) => (
                "different_process",
                None,
                Some(ordinal(ordinals, identity)?),
            ),
        };
        connection.execute(
            "INSERT INTO discovery_epochs VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                Word(discovery.id().get()),
                discovery.hinted_pid().get(),
                ordinal(ordinals, discovery.verified_parent())?,
                tag,
                failure,
                replacement
            ],
        )?;
    }
    Ok(())
}

pub(super) struct Identities {
    values: BTreeMap<i64, ProcessIdentity>,
    used: BTreeSet<i64>,
}
impl Identities {
    pub(super) fn resolve(&mut self, ordinal: i64) -> Result<ProcessIdentity> {
        let value = required(
            self.values.get(&ordinal).copied(),
            "dangling identity reference",
        )?;
        self.used.insert(ordinal);
        Ok(value)
    }
    pub(super) fn finish(self) -> Result<()> {
        if self.used.len() != self.values.len() {
            return malformed("unreferenced process identity");
        }
        Ok(())
    }
}

pub(super) fn read_identities(connection: &Connection, id: &[u8]) -> Result<Identities> {
    let mut statement = connection.prepare("SELECT ordinal,pid,birth_seconds,birth_nanos FROM process_identities WHERE attempt_id=?1 ORDER BY ordinal")?;
    let mut rows = statement.query([id])?;
    let mut values = BTreeMap::new();
    let mut unique = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let ordinal: i64 = row.get(0)?;
        if usize::try_from(ordinal).ok() != Some(values.len()) {
            return malformed("noncontiguous identity ordinals");
        }
        let birth = BirthToken::new(row.get::<_, Word>(2)?.0, row.get(3)?)
            .map_err(|_| ControllerStoreError::Malformed("invalid birth token"))?;
        let identity = ProcessIdentity::new(parse_pid(row.get(1)?)?, birth);
        if !unique.insert(identity) {
            return malformed("duplicate process identity");
        }
        values.insert(ordinal, identity);
    }
    Ok(Identities {
        values,
        used: BTreeSet::new(),
    })
}

pub(super) fn read(
    connection: &Connection,
    id: &[u8],
    identities: &mut Identities,
) -> Result<Option<OwnedProcessTree>> {
    let next = connection
        .query_row(
            "SELECT next_discovery FROM ownership WHERE attempt_id=?1",
            [id],
            |row| row.get::<_, Word>(0),
        )
        .optional()?;
    // Read every collection even when the ownership row is absent, to reject orphaned data.
    let owned = read_owned(connection, id, identities)?;
    let pending = read_pending(connection, id, identities)?;
    let unresolved = read_discoveries(connection, id, identities)?;
    let Some(next) = next else {
        if !owned.is_empty() || !pending.is_empty() || !unresolved.is_empty() {
            return malformed("ownership children without ownership");
        }
        return Ok(None);
    };
    Ok(Some(required(
        OwnedProcessTree::restore(next.0, owned, pending, unresolved),
        "invalid owned tree",
    )?))
}

fn read_owned(
    connection: &Connection,
    id: &[u8],
    identities: &mut Identities,
) -> Result<BTreeMap<ProcessIdentity, OwnedIdentityStatus>> {
    let mut statement = connection.prepare("SELECT identity_ordinal,status,current_parent_ordinal,failure FROM owned_identities WHERE attempt_id=?1")?;
    let mut rows = statement.query([id])?;
    let mut owned = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let identity = identities.resolve(row.get(0)?)?;
        let tag: String = row.get(1)?;
        let parent: Option<i64> = row.get(2)?;
        let failure: Option<String> = row.get(3)?;
        let status = match (tag.as_str(), parent, failure.as_deref()) {
            ("fresh", None, None) => OwnedIdentityStatus::NeedsFreshObservation,
            ("present", parent, None) => OwnedIdentityStatus::Present {
                current_parent: parent
                    .map(|ordinal| identities.resolve(ordinal))
                    .transpose()?,
            },
            ("absent", None, None) => OwnedIdentityStatus::VerifiedAbsent,
            ("uncertain", None, Some(failure)) => {
                OwnedIdentityStatus::Uncertain(parse_failure(failure)?)
            }
            _ => return malformed("invalid owned identity status"),
        };
        if owned.insert(identity, status).is_some() {
            return malformed("duplicate owned identity");
        }
    }
    Ok(owned)
}
fn read_pending(
    connection: &Connection,
    id: &[u8],
    identities: &mut Identities,
) -> Result<BTreeSet<ProcessIdentity>> {
    let mut statement =
        connection.prepare("SELECT parent_ordinal FROM pending_child_scans WHERE attempt_id=?1")?;
    let mut rows = statement.query([id])?;
    let mut pending = BTreeSet::new();
    while let Some(row) = rows.next()? {
        if !pending.insert(identities.resolve(row.get(0)?)?) {
            return malformed("duplicate pending scan");
        }
    }
    Ok(pending)
}
fn read_discoveries(
    connection: &Connection,
    id: &[u8],
    identities: &mut Identities,
) -> Result<BTreeMap<DiscoveryId, UnresolvedDiscovery>> {
    let mut statement = connection.prepare("SELECT discovery_id,hinted_pid,parent_ordinal,gap,failure,replacement_ordinal FROM discovery_epochs WHERE attempt_id=?1")?;
    let mut rows = statement.query([id])?;
    let mut unresolved = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let discovery = CandidateDiscovery::new(
            DiscoveryId::new(row.get::<_, Word>(0)?.0),
            parse_pid(row.get(1)?)?,
            identities.resolve(row.get(2)?)?,
        );
        let gap: String = row.get(3)?;
        let failure: Option<String> = row.get(4)?;
        let replacement: Option<i64> = row.get(5)?;
        let gap = match (gap.as_str(), failure.as_deref(), replacement) {
            ("not_observed", None, None) => DiscoveryGap::NotYetObserved,
            ("observation", Some(failure), None) => {
                DiscoveryGap::Observation(parse_failure(failure)?)
            }
            ("different_process", None, Some(ordinal)) => {
                DiscoveryGap::DifferentProcess(identities.resolve(ordinal)?)
            }
            _ => return malformed("invalid discovery gap"),
        };
        if unresolved
            .insert(discovery.id(), UnresolvedDiscovery { discovery, gap })
            .is_some()
        {
            return malformed("duplicate discovery epoch");
        }
    }
    Ok(unresolved)
}

fn parse_pid(value: u32) -> Result<ProcessId> {
    Ok(ProcessId::new(required(
        NonZeroU32::new(value),
        "zero PID",
    )?))
}
const fn failure_text(value: ObservationFailure) -> &'static str {
    match value {
        ObservationFailure::NoSuchProcess => "no_such_process",
        ObservationFailure::PermissionDenied => "permission",
        ObservationFailure::TemporarilyUnavailable => "unavailable",
        ObservationFailure::Malformed => "malformed",
    }
}
fn parse_failure(value: &str) -> Result<ObservationFailure> {
    match value {
        "no_such_process" => Ok(ObservationFailure::NoSuchProcess),
        "permission" => Ok(ObservationFailure::PermissionDenied),
        "unavailable" => Ok(ObservationFailure::TemporarilyUnavailable),
        "malformed" => Ok(ObservationFailure::Malformed),
        _ => malformed("unknown observation failure"),
    }
}
