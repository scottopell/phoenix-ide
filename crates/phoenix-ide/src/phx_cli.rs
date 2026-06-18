//! `phx` — the terminal-side companion to the Phoenix server.
//!
//! The server materializes a `phx` symlink to its own binary on the PTY `PATH`
//! (see [`crate::terminal::spawn::PtyEnvInjection`]), so `phx` is guaranteed
//! available in every terminal session without shipping a second artifact or
//! depending on a runtime that might be absent. When the binary is invoked
//! through that symlink it runs this thin client instead of starting the
//! server: it asks the local Phoenix server for shell-command suggestions and
//! prints them as click-to-run OSC 8 hyperlinks the terminal turns into
//! drop-onto-prompt affordances.
//!
//! Reaches the server over loopback using two vars the server injects into the
//! PTY env: `PHOENIX_API_URL` (where to call) and `PHOENIX_SUGGEST_TOKEN` (a
//! scoped capability that authorizes `/api/suggest` without the master
//! password).

use base64::Engine;
use std::io::{IsTerminal, Read};

/// True when this process is a `phx` CLI invocation rather than the server:
/// either argv[0]'s basename is `phx` (symlink path) or the first argument is
/// the explicit `suggest` subcommand (direct `phoenix_ide suggest …`, useful
/// for testing).
pub fn is_cli_invocation() -> bool {
    let mut args = std::env::args();
    let argv0 = args.next().unwrap_or_default();
    let invoked_as_phx = std::path::Path::new(&argv0)
        .file_name()
        .is_some_and(|n| n == "phx");
    invoked_as_phx || args.next().as_deref() == Some("suggest")
}

/// Run the `phx` client. Returns the process exit code.
pub async fn run() -> i32 {
    // reqwest's rustls backend needs a process crypto provider, same as the
    // server installs at startup. Harmless if one is already present.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    match run_inner().await {
        Ok(()) => 0,
        Err(msg) => {
            eprintln!("phx: {msg}");
            1
        }
    }
}

async fn run_inner() -> Result<(), String> {
    let query = read_query()?;
    if query.is_empty() {
        return Err("no query — pass it as arguments or pipe it on stdin".to_string());
    }

    let base = std::env::var("PHOENIX_API_URL").map_err(|_| {
        "PHOENIX_API_URL is not set — run this inside a Phoenix terminal".to_string()
    })?;
    let token = std::env::var("PHOENIX_SUGGEST_TOKEN").unwrap_or_default();

    // The local server speaks self-signed TLS on loopback; accept it (the URL
    // is the server's own loopback address, injected by the server itself).
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .post(format!("{}/api/suggest", base.trim_end_matches('/')))
        .header("X-Phoenix-Suggest-Token", token)
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("server returned {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("bad response: {e}"))?;
    let commands: Vec<String> = body
        .get("commands")
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();

    if commands.is_empty() {
        eprintln!("phx: no commands suggested");
        return Ok(());
    }

    eprintln!("Suggested commands (click ▶ to drop onto your prompt):");
    for command in &commands {
        println!("{}", osc8_run_link(command));
    }
    Ok(())
}

/// The query is the joined CLI arguments (after argv0 and an optional `suggest`
/// subcommand); if none are given, it is read from stdin so prompts can be
/// piped.
fn read_query() -> Result<String, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("suggest") {
        args.remove(0);
    }
    let joined = args.join(" ").trim().to_string();
    if !joined.is_empty() {
        return Ok(joined);
    }
    if std::io::stdin().is_terminal() {
        return Ok(String::new());
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| format!("reading stdin: {e}"))?;
    Ok(buf.trim().to_string())
}

/// Render a command as a clickable OSC 8 hyperlink with a `phxrun:<base64>`
/// target. The terminal intercepts `phxrun:` links and drops the decoded
/// command onto the shell prompt. Mirrors the emitter in `phoenix-client.py`.
fn osc8_run_link(command: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(command.as_bytes());
    format!("\x1b]8;;phxrun:{b64}\x1b\\▶ {command}\x1b]8;;\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc8_link_roundtrips() {
        let link = osc8_run_link("git worktree add ../wt main");
        assert!(link.starts_with("\x1b]8;;phxrun:"));
        assert!(link.ends_with("\x1b]8;;\x1b\\"));
        let b64 = link
            .strip_prefix("\x1b]8;;phxrun:")
            .and_then(|s| s.split('\x1b').next())
            .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(decoded, b"git worktree add ../wt main");
    }
}
