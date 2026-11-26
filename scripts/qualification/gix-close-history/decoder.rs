use std::{
    collections::BTreeSet,
    io::{self, Read},
    time::{Duration, Instant},
};

#[cfg(not(feature = "candidate"))]
include!("baseline.rs");

#[cfg(feature = "candidate")]
fn parse_target_head_reflog_roots(reflog: &[u8], deadline: Instant) -> Result<Vec<String>, String> {
    let mut roots = BTreeSet::new();
    for record in reflog.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        if Instant::now() >= deadline {
            return Err("deadline".into());
        }
        let entry = gix_ref::file::log::LineRef::from_bytes(record).map_err(|err| err.to_string())?;
        for oid in [entry.previous_oid, entry.new_oid] {
            let oid = std::str::from_utf8(oid).map_err(|err| err.to_string())?;
            if !oid.bytes().all(|byte| byte == b'0') {
                roots.insert(oid.to_owned());
                if roots.len() > 1024 {
                    return Err("distinct-root limit".into());
                }
            }
        }
    }
    Ok(roots.into_iter().collect())
}

fn main() -> Result<(), String> {
    let mut bytes = Vec::new();
    io::stdin().take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|err| err.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("capture limit".into());
    }
    let start = Instant::now();
    let deadline = if std::env::args_os().nth(1).is_some() { start } else { start + Duration::from_secs(5) };
    for oid in parse_target_head_reflog_roots(&bytes, deadline)? {
        println!("{oid}");
    }
    Ok(())
}
