use crate::hot_restart::Activation;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use serde::Serialize;
use ts_rs::TS;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum InstallationOwnership {
    LaunchdManaged,
    SystemdManaged,
    BareSupervisorManaged { supervisor_pid: u32 },
    Development,
    Unmanaged { reason: String },
    Ambiguous { reason: String },
    Unsupported { platform: String },
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
enum BareSupervisorEvidence {
    Absent,
    OwnsCurrentProcess { supervisor_pid: u32 },
    DoesNotOwnCurrentProcess,
    Unreadable(String),
}

#[cfg(target_os = "linux")]
#[derive(serde::Deserialize)]
struct BareRuntimeIdentity {
    version: String,
    git_sha: String,
}

#[cfg(target_os = "linux")]
#[derive(serde::Deserialize)]
struct BareChildIdentity {
    pid: u32,
    runtime: BareRuntimeIdentity,
}

#[cfg(target_os = "linux")]
#[derive(serde::Deserialize)]
struct BareStatusResponse {
    ok: bool,
    protocol_version: u32,
    supervisor_pid: u32,
    child: Option<BareChildIdentity>,
}

pub async fn detect(runtime_env: &PhoenixRuntimeEnvironment) -> InstallationOwnership {
    let bare = probe_bare_supervisor(runtime_env).await;
    classify(
        std::env::consts::OS,
        runtime_env.is_production(),
        crate::hot_restart::activation(),
        bare,
    )
}

fn classify(
    platform: &str,
    production: bool,
    activation: Activation,
    bare: BareSupervisorEvidence,
) -> InstallationOwnership {
    if !production {
        return InstallationOwnership::Development;
    }

    let socket_owner = match activation {
        Activation::Launchd => Some(InstallationOwnership::LaunchdManaged),
        Activation::Systemd => Some(InstallationOwnership::SystemdManaged),
        Activation::None => None,
    };

    match (socket_owner, bare) {
        (
            Some(owner),
            BareSupervisorEvidence::Absent | BareSupervisorEvidence::DoesNotOwnCurrentProcess,
        ) => owner,
        (Some(_), BareSupervisorEvidence::OwnsCurrentProcess { .. }) => {
            InstallationOwnership::Ambiguous {
                reason: "both socket activation and the bare supervisor claim the running process"
                    .to_string(),
            }
        }
        (Some(_), BareSupervisorEvidence::Unreadable(reason)) => InstallationOwnership::Ambiguous {
            reason: format!(
                "socket activation is present but bare-supervisor evidence is unreadable: {reason}"
            ),
        },
        (None, BareSupervisorEvidence::OwnsCurrentProcess { supervisor_pid }) => {
            InstallationOwnership::BareSupervisorManaged { supervisor_pid }
        }
        (None, BareSupervisorEvidence::Unreadable(reason)) => InstallationOwnership::Ambiguous {
            reason: format!("bare-supervisor ownership evidence is unreadable: {reason}"),
        },
        (
            None,
            BareSupervisorEvidence::Absent | BareSupervisorEvidence::DoesNotOwnCurrentProcess,
        ) if !matches!(platform, "macos" | "linux") => InstallationOwnership::Unsupported {
            platform: platform.to_string(),
        },
        (
            None,
            BareSupervisorEvidence::Absent | BareSupervisorEvidence::DoesNotOwnCurrentProcess,
        ) => InstallationOwnership::Unmanaged {
            reason: "the running process has no supported runtime-owner evidence".to_string(),
        },
    }
}

#[cfg(target_os = "linux")]
fn validate_bare_socket(path: &std::path::Path) -> Result<bool, String> {
    use std::os::linux::fs::MetadataExt;
    use std::os::unix::fs::FileTypeExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_socket()
        && metadata.st_uid() == unsafe { libc::geteuid() }
        && metadata.st_mode().trailing_zeros() >= 6
    {
        Ok(true)
    } else {
        Err("supervisor socket is not an owner-only socket owned by the Phoenix user".to_string())
    }
}

#[cfg(target_os = "linux")]
async fn request_bare_status(path: &std::path::Path) -> Result<(String, u32), String> {
    use std::os::unix::io::AsRawFd;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(path)
        .await
        .map_err(|error| error.to_string())?;
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>())
        .map_err(|error| error.to_string())?;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &raw mut length,
        )
    };
    if result != 0 || credentials.uid != unsafe { libc::geteuid() } {
        return Err("supervisor peer credentials do not match the Phoenix user".to_string());
    }
    let peer_pid =
        u32::try_from(credentials.pid).map_err(|_| "supervisor peer PID is invalid".to_string())?;
    stream
        .write_all(br#"{"protocol_version":1,"action":"status"}"#)
        .await
        .map_err(|error| error.to_string())?;
    stream.shutdown().await.map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .take(16 * 1024)
        .read_line(&mut response)
        .await
        .map_err(|error| error.to_string())?;
    Ok((response, peer_pid))
}

#[cfg(target_os = "linux")]
fn bare_evidence(
    response: &str,
    peer_pid: u32,
    process_pid: u32,
    parent_pid: u32,
) -> BareSupervisorEvidence {
    let status: BareStatusResponse = match serde_json::from_str(response) {
        Ok(status) => status,
        Err(error) => {
            return BareSupervisorEvidence::Unreadable(format!(
                "invalid supervisor status: {error}"
            ))
        }
    };
    if !status.ok || status.protocol_version != 1 || status.supervisor_pid != peer_pid {
        return BareSupervisorEvidence::Unreadable(
            "supervisor status did not match the authenticated protocol peer".to_string(),
        );
    }
    match status.child {
        Some(child) if child.pid == process_pid && parent_pid != peer_pid => {
            BareSupervisorEvidence::Unreadable(
                "supervisor claimed the current process but is not its direct parent".to_string(),
            )
        }
        Some(child)
            if child.pid == process_pid
                && child.runtime.version == env!("CARGO_PKG_VERSION")
                && child.runtime.git_sha == env!("PHOENIX_GIT_SHA") =>
        {
            BareSupervisorEvidence::OwnsCurrentProcess {
                supervisor_pid: status.supervisor_pid,
            }
        }
        _ => BareSupervisorEvidence::DoesNotOwnCurrentProcess,
    }
}

#[cfg(target_os = "linux")]
async fn probe_bare_supervisor(runtime_env: &PhoenixRuntimeEnvironment) -> BareSupervisorEvidence {
    use tokio::time::{timeout, Duration};

    let socket_path = runtime_env.phoenix_home().join("run/supervisor.sock");
    match validate_bare_socket(&socket_path) {
        Ok(true) => {}
        Ok(false) => return BareSupervisorEvidence::Absent,
        Err(reason) => return BareSupervisorEvidence::Unreadable(reason),
    }
    match timeout(
        Duration::from_millis(500),
        request_bare_status(&socket_path),
    )
    .await
    {
        Ok(Ok((response, peer_pid))) => bare_evidence(
            &response,
            peer_pid,
            std::process::id(),
            unsafe { libc::getppid() }.cast_unsigned(),
        ),
        Ok(Err(reason)) => BareSupervisorEvidence::Unreadable(reason),
        Err(_) => {
            BareSupervisorEvidence::Unreadable("supervisor status probe timed out".to_string())
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn probe_bare_supervisor(
    _runtime_env: &PhoenixRuntimeEnvironment,
) -> std::future::Ready<BareSupervisorEvidence> {
    std::future::ready(BareSupervisorEvidence::Absent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_macos_process_is_not_launchd_managed() {
        assert_eq!(
            classify(
                "macos",
                true,
                Activation::None,
                BareSupervisorEvidence::Absent
            ),
            InstallationOwnership::Unmanaged {
                reason: "the running process has no supported runtime-owner evidence".to_string(),
            }
        );
    }

    #[test]
    fn linux_host_without_process_owner_evidence_is_not_systemd_managed() {
        assert!(matches!(
            classify(
                "linux",
                true,
                Activation::None,
                BareSupervisorEvidence::Absent
            ),
            InstallationOwnership::Unmanaged { .. }
        ));
    }

    #[test]
    fn authenticated_live_bare_supervisor_is_positive_evidence() {
        assert_eq!(
            classify(
                "linux",
                true,
                Activation::None,
                BareSupervisorEvidence::OwnsCurrentProcess { supervisor_pid: 42 },
            ),
            InstallationOwnership::BareSupervisorManaged { supervisor_pid: 42 }
        );
    }

    #[test]
    fn contradictory_or_unreadable_evidence_is_ambiguous() {
        assert!(matches!(
            classify(
                "linux",
                true,
                Activation::Systemd,
                BareSupervisorEvidence::OwnsCurrentProcess { supervisor_pid: 42 },
            ),
            InstallationOwnership::Ambiguous { .. }
        ));
        assert!(matches!(
            classify(
                "linux",
                true,
                Activation::None,
                BareSupervisorEvidence::Unreadable("denied".to_string()),
            ),
            InstallationOwnership::Ambiguous { .. }
        ));
        assert!(matches!(
            classify(
                "linux",
                true,
                Activation::Systemd,
                BareSupervisorEvidence::Unreadable("denied".to_string()),
            ),
            InstallationOwnership::Ambiguous { .. }
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unrelated_bare_supervisor_is_not_owner_evidence() {
        let response = serde_json::json!({
            "ok": true,
            "protocol_version": 1,
            "supervisor_pid": 42,
            "child": null,
        })
        .to_string();
        assert_eq!(
            bare_evidence(&response, 42, 100, 7),
            BareSupervisorEvidence::DoesNotOwnCurrentProcess
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn false_bare_claim_is_unreadable_evidence() {
        let response = serde_json::json!({
            "ok": true,
            "protocol_version": 1,
            "supervisor_pid": 42,
            "child": {
                "pid": 100,
                "runtime": {
                    "version": env!("CARGO_PKG_VERSION"),
                    "git_sha": env!("PHOENIX_GIT_SHA"),
                },
            },
        })
        .to_string();
        assert!(matches!(
            bare_evidence(&response, 42, 100, 7),
            BareSupervisorEvidence::Unreadable(_)
        ));
    }

    #[test]
    fn wire_shape_preserves_state_and_evidence() {
        assert_eq!(
            serde_json::to_value(InstallationOwnership::BareSupervisorManaged {
                supervisor_pid: 42,
            })
            .unwrap(),
            serde_json::json!({ "kind": "bare_supervisor_managed", "supervisor_pid": 42 })
        );
        assert_eq!(
            serde_json::to_value(InstallationOwnership::Ambiguous {
                reason: "conflict".to_string(),
            })
            .unwrap(),
            serde_json::json!({ "kind": "ambiguous", "reason": "conflict" })
        );
    }

    #[test]
    fn development_and_unsupported_are_explicit() {
        assert_eq!(
            classify(
                "linux",
                false,
                Activation::None,
                BareSupervisorEvidence::Absent
            ),
            InstallationOwnership::Development
        );
        assert_eq!(
            classify(
                "linux",
                false,
                Activation::Systemd,
                BareSupervisorEvidence::Absent
            ),
            InstallationOwnership::Development
        );
        assert_eq!(
            classify(
                "linux",
                false,
                Activation::None,
                BareSupervisorEvidence::OwnsCurrentProcess { supervisor_pid: 42 }
            ),
            InstallationOwnership::Development
        );
        assert_eq!(
            classify(
                "windows",
                true,
                Activation::None,
                BareSupervisorEvidence::Absent
            ),
            InstallationOwnership::Unsupported {
                platform: "windows".to_string()
            }
        );
    }
}
