//! Local-only "reveal in the OS file manager" support.
//!
//! `POST /api/files/reveal` opens the *containing folder* of a path in the
//! server host's file manager. The window opens on the server host's desktop —
//! nowhere else — so the action is meaningful only when the requesting browser
//! runs on the same physical machine as the server. The endpoint enforces this
//! structurally: it refuses any request whose connection did not originate from
//! this host, and the capability is advertised to the UI as
//! [`DeploymentInfo::local_access`](super::deployment::DeploymentInfo).
//!
//! Reveal opens a *directory*, never a file: opening a file by association would
//! launch it (`open foo.dmg` mounts it, `xdg-open script.desktop` executes it).
//! Opening a directory cannot launch anything, so an arbitrary path is harmless
//! — at worst it opens a folder the local caller already has filesystem access
//! to. That property, not a path allowlist, is what makes the endpoint safe.

use super::handlers::AppError;
use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct RevealRequest {
    /// Absolute path whose containing folder should be revealed.
    pub path: String,
}

/// Whether a connection from `peer` originated on this same host.
///
/// True for loopback, and for any address that matches one of this machine's
/// own network interfaces: when a host connects to its own LAN address (e.g. a
/// browser opening `https://my-host.local:8031` on the machine that serves it),
/// the server observes its *own* interface address as the peer. A genuinely
/// remote host presents a different source address and is rejected. The peer is
/// the completed-handshake TCP source, so it cannot be spoofed to the server's
/// own address from another machine.
pub fn peer_is_local(peer: IpAddr) -> bool {
    peer.is_loopback() || host_addresses().into_iter().any(|ip| ip == peer)
}

/// This machine's interface addresses. Enumerated per call rather than cached:
/// DHCP renewal or a VPN coming up changes the set, and the syscall is cheap. An
/// enumeration failure yields an empty set (only loopback will then match),
/// never a panic.
fn host_addresses() -> Vec<IpAddr> {
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces.into_iter().map(|i| i.ip()).collect(),
        Err(e) => {
            tracing::debug!(error = %e, "could not enumerate host interfaces for local-access check");
            Vec::new()
        }
    }
}

/// `POST /api/files/reveal` — open a path's containing folder in the server
/// host's file manager. Refuses non-local callers (403).
pub async fn reveal_path(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<RevealRequest>,
) -> Result<StatusCode, AppError> {
    if !peer_is_local(peer.ip()) {
        return Err(AppError::Forbidden(
            "reveal is available only to a browser running on the server host".to_string(),
        ));
    }

    let path = PathBuf::from(&body.path);
    // A directory reveals itself; a file reveals the folder that contains it.
    let folder = if path.is_dir() {
        path
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| AppError::BadRequest("path has no containing folder".to_string()))?
    };
    if !folder.is_dir() {
        return Err(AppError::NotFound(format!(
            "folder does not exist: {}",
            folder.display()
        )));
    }

    open_folder(&folder)
        .await
        .map_err(|e| AppError::Internal(format!("failed to open file manager: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Open `folder` in the host's native file manager. Spawned via the platform's
/// hand-off launcher (`open`/`xdg-open`/`explorer`), which returns once the
/// running file manager has been signalled — it does not block on a window.
async fn open_folder(folder: &Path) -> std::io::Result<()> {
    use tokio::process::Command;

    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");

    cmd.arg(folder);
    let status = cmd.status().await?;
    // `explorer` is documented to return non-zero even on success; treat only
    // the platforms with well-behaved exit codes as failable.
    #[cfg(not(target_os = "windows"))]
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "file manager exited with {status}"
        )));
    }
    #[cfg(target_os = "windows")]
    let _ = status;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn loopback_is_local() {
        assert!(peer_is_local(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(peer_is_local(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn own_interface_address_is_local() {
        // Every host has at least loopback; pick a non-loopback interface if one
        // exists and assert it is recognised as local. Skips cleanly on hosts
        // that expose only loopback (some CI sandboxes).
        let Some(own) = host_addresses().into_iter().find(|ip| !ip.is_loopback()) else {
            return;
        };
        assert!(peer_is_local(own));
    }

    #[test]
    fn documentation_example_remote_address_is_not_local() {
        // A TEST-NET-2 address (RFC 5737) will not be a real interface here.
        let remote = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));
        assert!(!peer_is_local(remote));
    }
}
