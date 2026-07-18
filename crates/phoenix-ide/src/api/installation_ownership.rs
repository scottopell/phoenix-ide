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
        ) if !production => InstallationOwnership::Development,
        (
            None,
            BareSupervisorEvidence::Absent | BareSupervisorEvidence::DoesNotOwnCurrentProcess,
        ) => InstallationOwnership::Unmanaged {
            reason: "the running process has no supported runtime-owner evidence".to_string(),
        },
    }
}

#[cfg(target_os = "linux")]
async fn probe_bare_supervisor(runtime_env: &PhoenixRuntimeEnvironment) -> BareSupervisorEvidence {
    use serde::Deserialize;
    use std::os::linux::fs::MetadataExt;
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::io::AsRawFd;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::time::{timeout, Duration};

    #[derive(Deserialize)]
    struct RuntimeIdentity {
        version: String,
        git_sha: String,
    }

    #[derive(Deserialize)]
    struct ChildIdentity {
        pid: u32,
        runtime: RuntimeIdentity,
    }

    #[derive(Deserialize)]
    struct StatusResponse {
        ok: bool,
        protocol_version: u32,
        supervisor_pid: u32,
        child: Option<ChildIdentity>,
    }

    const PROTOCOL_VERSION: u32 = 1;
    const MAX_RESPONSE_BYTES: u64 = 16 * 1024;
    const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

    let socket_path = runtime_env.phoenix_home().join("run/supervisor.sock");
    let metadata = match std::fs::symlink_metadata(&socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BareSupervisorEvidence::Absent
        }
        Err(error) => return BareSupervisorEvidence::Unreadable(error.to_string()),
    };
    if !metadata.file_type().is_socket()
        || metadata.st_uid() != unsafe { libc::geteuid() }
        || metadata.st_mode() & 0o077 != 0
    {
        return BareSupervisorEvidence::Unreadable(
            "supervisor socket is not an owner-only socket owned by the Phoenix user".to_string(),
        );
    }

    let probe = async {
        let mut stream = UnixStream::connect(&socket_path).await?;
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
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
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "supervisor peer credentials do not match the Phoenix user",
            ));
        }
        stream
            .write_all(br#"{"protocol_version":1,"action":"status"}"#)
            .await?;
        stream.shutdown().await?;
        let mut response = String::new();
        BufReader::new(stream)
            .take(MAX_RESPONSE_BYTES)
            .read_line(&mut response)
            .await?;
        Ok::<_, std::io::Error>((response, credentials.pid))
    };

    let (response, peer_pid) = match timeout(PROBE_TIMEOUT, probe).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return BareSupervisorEvidence::Unreadable(error.to_string()),
        Err(_) => {
            return BareSupervisorEvidence::Unreadable(
                "supervisor status probe timed out".to_string(),
            )
        }
    };
    let status: StatusResponse = match serde_json::from_str(&response) {
        Ok(status) => status,
        Err(error) => {
            return BareSupervisorEvidence::Unreadable(format!(
                "invalid supervisor status: {error}"
            ))
        }
    };
    if !status.ok
        || status.protocol_version != PROTOCOL_VERSION
        || status.supervisor_pid != peer_pid as u32
    {
        return BareSupervisorEvidence::Unreadable(
            "supervisor status did not match the authenticated protocol peer".to_string(),
        );
    }
    match status.child {
        Some(child)
            if child.pid == std::process::id()
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
