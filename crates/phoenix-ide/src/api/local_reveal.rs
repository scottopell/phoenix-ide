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
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct RevealRequest {
    /// Absolute path whose containing folder should be revealed.
    pub path: String,
}

/// Whether the requesting client is on the server host.
///
/// The connection peer is the primary signal: loopback, or an address matching
/// one of this machine's own interfaces (a host reaching the server by its LAN
/// name — `https://my-host.local:8031` — sees the server observe its *own*
/// interface address as the peer). A genuinely remote host presents a different
/// source address.
///
/// A *loopback* peer is ambiguous: it is either genuine localhost, or a
/// same-host proxy forwarding for someone else (the Vite dev proxy forwards
/// `/api` to Phoenix on `127.0.0.1`; a same-host reverse proxy does likewise).
/// So when the peer is loopback and an `X-Forwarded-For` header is present, the
/// original client is the first entry and locality is decided from *it*. This is
/// not spoofable from off-host: reaching this branch requires a loopback peer,
/// which a remote attacker connecting directly cannot present — their forged
/// `X-Forwarded-For` is ignored because their peer is non-loopback. A loopback
/// peer with no `X-Forwarded-For` is treated as genuine localhost.
pub fn client_is_local(peer: IpAddr, headers: &HeaderMap) -> bool {
    if peer.is_loopback() {
        match forwarded_client(headers) {
            Some(client) => return ip_is_local(client),
            // Forwarded-for present but unparseable: a proxy hop we cannot
            // resolve — refuse rather than assume local.
            None if headers.contains_key("x-forwarded-for") => return false,
            None => return true,
        }
    }
    ip_is_local(peer)
}

/// The original client address from `X-Forwarded-For` (its first entry), or None
/// when the header is absent or its leading entry does not parse as an IP.
fn forwarded_client(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")?
        .to_str()
        .ok()?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Whether `ip` belongs to this host: loopback or one of its interface addresses.
fn ip_is_local(ip: IpAddr) -> bool {
    ip.is_loopback() || host_addresses().into_iter().any(|own| own == ip)
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
    headers: HeaderMap,
    Json(body): Json<RevealRequest>,
) -> Result<StatusCode, AppError> {
    if !client_is_local(peer.ip(), &headers) {
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

    const REMOTE: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)); // TEST-NET-2

    fn xff(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", value.parse().unwrap());
        h
    }

    #[test]
    fn loopback_without_forwarded_for_is_local() {
        let none = HeaderMap::new();
        assert!(client_is_local(IpAddr::V4(Ipv4Addr::LOCALHOST), &none));
        assert!(client_is_local(
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            &none
        ));
    }

    #[test]
    fn own_interface_address_is_local() {
        // Every host has at least loopback; pick a non-loopback interface if one
        // exists and assert it is recognised as local. Skips cleanly on hosts
        // that expose only loopback (some CI sandboxes).
        let Some(own) = host_addresses().into_iter().find(|ip| !ip.is_loopback()) else {
            return;
        };
        assert!(client_is_local(own, &HeaderMap::new()));
    }

    #[test]
    fn remote_peer_is_not_local() {
        assert!(!client_is_local(REMOTE, &HeaderMap::new()));
    }

    #[test]
    fn proxied_remote_client_is_not_local() {
        // A loopback peer (our dev/reverse proxy) forwarding a remote client must
        // not be treated as local — this is the Vite-proxy bypass case.
        assert!(!client_is_local(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            &xff("198.51.100.7")
        ));
        assert!(!client_is_local(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            &xff("198.51.100.7, 127.0.0.1")
        ));
    }

    #[test]
    fn proxied_local_client_is_local() {
        // The proxy forwarding for genuine localhost stays local.
        assert!(client_is_local(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            &xff("127.0.0.1")
        ));
    }

    #[test]
    fn remote_peer_cannot_spoof_via_forwarded_for() {
        // A direct remote connection (non-loopback peer) never consults XFF, so a
        // forged header does not grant local access.
        assert!(!client_is_local(REMOTE, &xff("127.0.0.1")));
    }

    #[test]
    fn loopback_peer_with_unparseable_forwarded_for_is_rejected() {
        assert!(!client_is_local(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            &xff("not-an-ip")
        ));
    }
}
