//! Bounded raw-byte payload used by the early in-process bash launcher.
//! Payloads are supplied on stdin; argv and the bootstrap environment carry no user data.

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::path::PathBuf;

const MAGIC: &[u8; 4] = b"PHXB";
const VERSION: u8 = 1;
pub const MAX_PAYLOAD: usize = 1024 * 1024;
const MAX_FIELD: usize = 256 * 1024;
const MAX_ENV: usize = 512;

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashLaunchMode {
    Direct,
    ExploreReadOnly,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashLaunchPayload {
    pub mode: BashLaunchMode,
    pub command: OsString,
    pub working_dir: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
}

#[cfg(unix)]
impl BashLaunchPayload {
    /// Encodes a bounded launch payload without executing or exposing its contents.
    ///
    /// # Errors
    /// Returns an error if any field, environment count, or total payload exceeds its bound.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        use std::os::unix::ffi::OsStrExt;
        if self.environment.len() > MAX_ENV {
            return Err("too many environment entries".into());
        }
        let command = self.command.as_os_str().as_bytes();
        let working_dir = self.working_dir.as_os_str().as_bytes();
        let mut encoded_len = 18usize;
        for field in
            [command, working_dir]
                .into_iter()
                .chain(self.environment.iter().flat_map(|(key, value)| {
                    [key.as_os_str().as_bytes(), value.as_os_str().as_bytes()]
                }))
        {
            if field.len() > MAX_FIELD {
                return Err("bash launch field exceeds limit".into());
            }
            encoded_len = encoded_len
                .checked_add(4 + field.len())
                .filter(|length| *length <= MAX_PAYLOAD)
                .ok_or("bash launch payload exceeds limit")?;
        }
        let mut out = Vec::with_capacity(encoded_len);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(match self.mode {
            BashLaunchMode::Direct => 0,
            BashLaunchMode::ExploreReadOnly => 1,
        });
        put_field(&mut out, command)?;
        put_field(&mut out, working_dir)?;
        let environment_count = u32::try_from(self.environment.len())
            .map_err(|_| "bash launch environment count exceeds limit")?;
        out.extend_from_slice(&environment_count.to_be_bytes());
        for (key, value) in &self.environment {
            put_field(&mut out, key.as_os_str().as_bytes())?;
            put_field(&mut out, value.as_os_str().as_bytes())?;
        }
        Ok(out)
    }
}

#[cfg(unix)]
fn put_field(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_FIELD {
        return Err("bash launch field exceeds limit".into());
    }
    let length = u32::try_from(bytes.len()).map_err(|_| "bash launch field exceeds limit")?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

#[cfg(unix)]
fn decode(bytes: &[u8]) -> Result<BashLaunchPayload, String> {
    use std::os::unix::ffi::OsStringExt;
    if bytes.len() > MAX_PAYLOAD || bytes.get(..4) != Some(MAGIC) || bytes.get(4) != Some(&VERSION)
    {
        return Err("invalid or oversized bash launch payload".into());
    }
    let mode = match bytes.get(5) {
        Some(0) => BashLaunchMode::Direct,
        Some(1) => BashLaunchMode::ExploreReadOnly,
        _ => return Err("invalid bash launch mode".into()),
    };
    let mut cursor = 6;
    let command = OsString::from_vec(take_field(bytes, &mut cursor)?);
    let working_dir = PathBuf::from(OsString::from_vec(take_field(bytes, &mut cursor)?));
    let count = take_u32(bytes, &mut cursor)? as usize;
    if count > MAX_ENV {
        return Err("too many environment entries".into());
    }
    let mut environment = Vec::with_capacity(count);
    for _ in 0..count {
        let key = OsString::from_vec(take_field(bytes, &mut cursor)?);
        let value = OsString::from_vec(take_field(bytes, &mut cursor)?);
        if key.is_empty()
            || key.as_bytes().contains(&b'=')
            || key.as_bytes().contains(&0)
            || value.as_bytes().contains(&0)
        {
            return Err("invalid environment entry in bash launch payload".into());
        }
        environment.push((key, value));
    }
    if cursor != bytes.len()
        || command.as_bytes().contains(&0)
        || working_dir.as_os_str().as_bytes().contains(&0)
    {
        return Err("trailing or invalid bytes in bash launch payload".into());
    }
    Ok(BashLaunchPayload {
        mode,
        command,
        working_dir,
        environment,
    })
}

#[cfg(unix)]
fn take_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let end = cursor.checked_add(4).ok_or("invalid bash launch payload")?;
    let raw: [u8; 4] = bytes
        .get(*cursor..end)
        .ok_or("truncated bash launch payload")?
        .try_into()
        .map_err(|_| "truncated bash launch payload")?;
    *cursor = end;
    Ok(u32::from_be_bytes(raw))
}

#[cfg(unix)]
fn take_field(bytes: &[u8], cursor: &mut usize) -> Result<Vec<u8>, String> {
    let length = take_u32(bytes, cursor)? as usize;
    if length > MAX_FIELD {
        return Err("bash launch field exceeds limit".into());
    }
    let end = cursor
        .checked_add(length)
        .ok_or("invalid bash launch payload")?;
    let value = bytes
        .get(*cursor..end)
        .ok_or("truncated bash launch payload")?
        .to_vec();
    *cursor = end;
    Ok(value)
}

/// Dispatch only from the executable's earliest entry point. Invalid or incomplete
/// input exits without executing user code.
#[cfg(unix)]
pub fn early_exec_from_stdin() -> ! {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut bytes = Vec::new();
    let read = std::io::stdin()
        .take((MAX_PAYLOAD + 1) as u64)
        .read_to_end(&mut bytes);
    let result = read
        .map_err(|error| error.to_string())
        .and_then(|_| decode(&bytes));
    let payload = match result {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("phoenix bash launcher rejected payload: {error}");
            std::process::exit(126);
        }
    };
    // The bootstrap stdin is a pipe containing the private launch payload. The
    // user command's stdin contract is /dev/null, not that exhausted pipe.
    let null_stdin = match std::fs::File::open("/dev/null") {
        Ok(file) => file,
        Err(error) => {
            eprintln!("phoenix bash launcher could not restore stdin: {error}");
            std::process::exit(126);
        }
    };
    // SAFETY: dup2 replaces only this process's stdin descriptor before exec.
    let duplicated = unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&null_stdin), 0) };
    if duplicated == -1 {
        let error = std::io::Error::last_os_error();
        eprintln!("phoenix bash launcher could not restore stdin: {error}");
        std::process::exit(126);
    }
    drop(null_stdin);
    if payload.mode == BashLaunchMode::ExploreReadOnly {
        let command = match payload.command.into_string() {
            Ok(command) => command,
            Err(_) => {
                eprintln!("phoenix bash launcher rejected non-UTF-8 Explore command");
                std::process::exit(126);
            }
        };
        for (key, value) in payload.environment {
            std::env::set_var(key, value);
        }
        super::apply_explore_read_only_from_env(&command);
    }
    let mut command = std::process::Command::new("bash");
    command
        .arg("-c")
        .arg(&payload.command)
        .current_dir(payload.working_dir)
        .env_clear();
    for (key, value) in payload.environment {
        command.env(key, value);
    }
    let error = command.exec();
    eprintln!("phoenix bash launcher exec failed: {error}");
    std::process::exit(126)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn round_trips_non_utf8_payload() {
        let payload = BashLaunchPayload {
            mode: BashLaunchMode::Direct,
            command: OsString::from_vec(b"echo \xff".to_vec()),
            working_dir: PathBuf::from("/tmp"),
            environment: vec![("PATH".into(), "/bin".into())],
        };
        assert_eq!(decode(&payload.encode().unwrap()).unwrap(), payload);
    }

    #[test]
    fn payload_mode_is_closed_and_round_trips() {
        let payload = BashLaunchPayload {
            mode: BashLaunchMode::ExploreReadOnly,
            command: "echo explore".into(),
            working_dir: "/tmp".into(),
            environment: vec![("PHOENIX_BASH_LAUNCH_UUID".into(), "launch-id".into())],
        };
        let encoded = payload.encode().unwrap();
        assert_eq!(decode(&encoded).unwrap(), payload);
        let mut invalid_mode = encoded;
        invalid_mode[5] = 2;
        assert!(decode(&invalid_mode).is_err());
    }

    #[test]
    fn rejects_truncated_trailing_and_oversized_payloads() {
        let payload = BashLaunchPayload {
            mode: BashLaunchMode::Direct,
            command: "true".into(),
            working_dir: "/tmp".into(),
            environment: vec![],
        }
        .encode()
        .unwrap();
        assert!(decode(&payload[..payload.len() - 1]).is_err());
        let mut trailing = payload;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        assert!(decode(&vec![0; MAX_PAYLOAD + 1]).is_err());
    }

    #[test]
    fn encode_rejects_aggregate_over_budget_before_building_output() {
        let payload = BashLaunchPayload {
            mode: BashLaunchMode::Direct,
            command: OsString::from("c".repeat(MAX_FIELD)),
            working_dir: PathBuf::from(format!("/{}", "d".repeat(MAX_FIELD - 1))),
            environment: vec![
                (OsString::from("A"), OsString::from("v".repeat(MAX_FIELD))),
                (OsString::from("B"), OsString::from("w".repeat(MAX_FIELD))),
            ],
        };
        // Four individually valid 256 KiB fields exceed the 1 MiB aggregate
        // bound (command, working directory, and two environment values).
        assert_eq!(
            payload.encode().unwrap_err(),
            String::from("bash launch payload exceeds limit")
        );
    }
}
