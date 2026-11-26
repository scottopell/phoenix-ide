//! Disposable hosted qualification probe. Not used by Close.

use std::{
    collections::BTreeSet,
    io::{self, Read},
    path::PathBuf,
    time::{Duration, Instant},
};

use gix::{ObjectId, Repository};
use gix_ref::file::log::LineRef;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const CAPTURE_LIMIT: u64 = 16 * 1024 * 1024;
const REFLOG_ROOT_LIMIT: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
enum Head {
    Attached(Vec<u8>, ObjectId),
    Detached(ObjectId),
    Unborn(Vec<u8>),
}

fn head(repo: &Repository) -> Result<Head> {
    use gix_ref::Target;
    let mut head = repo.find_reference("HEAD")?;
    match head.inner().target.clone() {
        Target::Object(id) => Ok(Head::Detached(id)),
        Target::Symbolic(name) => {
            let name_bytes = name.as_bstr().to_vec();
            if name_bytes.starts_with(b"refs/heads/")
                && repo.try_find_reference(name.as_bstr())?.is_none()
            {
                Ok(Head::Unborn(name_bytes))
            } else {
                Ok(Head::Attached(name_bytes, head.follow_to_object()?.detach()))
            }
        }
    }
}

fn reflog_roots(
    bytes: &[u8],
    hash: gix::hash::Kind,
    deadline: Instant,
) -> Result<BTreeSet<ObjectId>> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err("incomplete HEAD reflog record".into());
    }
    let mut roots = BTreeSet::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        check_deadline(deadline)?;
        let record = LineRef::from_bytes(line)?;
        for id in [record.previous_oid(), record.new_oid()] {
            if id.kind() != hash {
                return Err("HEAD reflog hash does not match repository".into());
            }
            if !id.is_null() && roots.insert(id) && roots.len() > REFLOG_ROOT_LIMIT {
                return Err("HEAD reflog exceeds distinct-root limit".into());
            }
        }
    }
    Ok(roots)
}

fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err("observation deadline expired".into());
    }
    Ok(())
}

fn inventory(
    repo: &Repository,
    roots: BTreeSet<ObjectId>,
    deadline: Instant,
    budget: usize,
) -> Result<BTreeSet<ObjectId>> {
    if roots.is_empty() {
        return Ok(roots);
    }
    let mut durable = BTreeSet::new();
    let refs = repo.references()?;
    let mut remaining = budget;
    for iter in [refs.local_branches()?, refs.remote_branches()?, refs.tags()?] {
        for reference in iter {
            charge(deadline, &mut remaining)?;
            add_durable(repo, reference?, &mut durable)?;
        }
    }
    if let Some(stash) = repo.try_find_reference("refs/stash")?
        && stash.name().as_bstr().as_slice() == b"refs/stash"
    {
        charge(deadline, &mut remaining)?;
        add_durable(repo, stash, &mut durable)?;
    }
    let mut preserved = BTreeSet::new();
    // Two complete walks avoid the date-based cutoff of gix's with_hidden().
    for item in repo.rev_walk(durable).use_commit_graph(false).all()? {
        charge(deadline, &mut remaining)?;
        preserved.insert(item?.id);
    }
    let commits = roots
        .into_iter()
        .map(|id| Ok(repo.find_object(id)?.peel_to_commit()?.id))
        .collect::<Result<Vec<_>>>()?;
    let mut loss = BTreeSet::new();
    for item in repo
        .rev_walk(commits)
        .use_commit_graph(false)
        .selected(move |id| !preserved.contains(&id.to_owned()))?
    {
        charge(deadline, &mut remaining)?;
        loss.insert(item?.id);
    }
    check_deadline(deadline)?;
    Ok(loss)
}

fn charge(deadline: Instant, remaining: &mut usize) -> Result<()> {
    check_deadline(deadline)?;
    *remaining = remaining.checked_sub(1).ok_or("graph/ref budget exhausted")?;
    Ok(())
}

fn add_durable(
    repo: &Repository,
    mut reference: gix::Reference<'_>,
    tips: &mut BTreeSet<ObjectId>,
) -> Result<()> {
    let target = reference.follow_to_object()?.detach();
    let object = repo.find_object(target)?.peel_tags_to_end()?;
    if object.kind.is_commit() {
        tips.insert(object.id);
    }
    Ok(())
}

fn main() -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut args = std::env::args_os().skip(1);
    let worktree = PathBuf::from(args.next().ok_or("expected worktree path")?);
    let budget = args
        .next()
        .map(|value| value.to_string_lossy().parse::<usize>())
        .transpose()?
        .unwrap_or(100_000);
    let repo = gix::open_opts(worktree, gix::open::Options::isolated())?;
    if let Some(storage) = repo.config_snapshot().string("extensions.refStorage")
        && storage.as_slice() != b"files"
    {
        return Err("unsupported ref storage".into());
    }
    let observed_head = head(&repo)?;
    // Accept captured bytes, not a path to reopen through gix.
    let mut captured = Vec::new();
    io::stdin().take(CAPTURE_LIMIT + 1).read_to_end(&mut captured)?;
    if captured.len() as u64 > CAPTURE_LIMIT {
        return Err("HEAD reflog capture exceeds byte limit".into());
    }
    let mut roots = reflog_roots(&captured, repo.object_hash(), deadline)?;
    if let Head::Attached(_, id) | Head::Detached(id) = observed_head {
        roots.insert(id);
    }
    let loss = inventory(&repo, roots, deadline, budget)?;
    if head(&repo)? != observed_head {
        return Err("HEAD changed during observation".into());
    }
    check_deadline(deadline)?;
    match observed_head {
        Head::Attached(name, id) => println!("attached {} {id}", std::str::from_utf8(&name)?),
        Head::Detached(id) => println!("detached {id}"),
        Head::Unborn(name) => println!("unborn {}", std::str::from_utf8(&name)?),
    }
    for id in loss {
        println!("{id}");
    }
    Ok(())
}
