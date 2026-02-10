use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::api::AppState;

const AUTH_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

// Auth state shared across requests
pub struct AuthState {
    pub status: AuthStatus,
    pub process: Option<DdtoolProcess>,
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            status: AuthStatus::NotRequired,
            process: None,
        }
    }
}

#[derive(Clone)]
pub enum AuthStatus {
    NotRequired,   // AI_GATEWAY_ENABLED=false
    Authenticated, // Token exists and valid
    Required,      // Needs authentication
    InProgress,    // ddtool running, waiting for user
    Failed(String), // Error occurred
}

pub struct DdtoolProcess {
    pub child: Child,
    pub oauth_url: String,
    pub device_code: String,
    pub started_at: Instant,
}

#[derive(Serialize)]
pub struct AuthStatusResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    oauth_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl AuthStatusResponse {
    fn from_status(status: &AuthStatus, process: &Option<DdtoolProcess>) -> Self {
        match status {
            AuthStatus::NotRequired => Self {
                status: "not_required".to_string(),
                oauth_url: None,
                device_code: None,
                error: None,
            },
            AuthStatus::Authenticated => Self {
                status: "authenticated".to_string(),
                oauth_url: None,
                device_code: None,
                error: None,
            },
            AuthStatus::Required => Self {
                status: "required".to_string(),
                oauth_url: None,
                device_code: None,
                error: None,
            },
            AuthStatus::InProgress => {
                let (url, code) = process.as_ref().map_or((None, None), |p| {
                    (Some(p.oauth_url.clone()), Some(p.device_code.clone()))
                });
                Self {
                    status: "in_progress".to_string(),
                    oauth_url: url,
                    device_code: code,
                    error: None,
                }
            }
            AuthStatus::Failed(err) => Self {
                status: "failed".to_string(),
                oauth_url: None,
                device_code: None,
                error: Some(err.clone()),
            },
        }
    }
}

/// Check if AI Gateway is enabled
fn is_ai_gateway_enabled() -> bool {
    std::env::var("AI_GATEWAY_ENABLED")
        .ok()
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(false)
}

/// Check if ddtool is already authenticated by trying to get a token
fn check_auth_exists() -> bool {
    let datacenter = std::env::var("AI_GATEWAY_DATACENTER")
        .expect("AI_GATEWAY_DATACENTER must be set when using AI Gateway mode");
    let service_name = std::env::var("AI_GATEWAY_SERVICE")
        .expect("AI_GATEWAY_SERVICE must be set when using AI Gateway mode");

    // Try to get a token - if successful, we're authenticated
    let output = Command::new("ddtool")
        .args(&[
            "auth",
            "token",
            &service_name,
            "--datacenter",
            &datacenter,
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            // Check if output looks like a JWT token (starts with eyJ...)
            let token = String::from_utf8_lossy(&out.stdout);
            let token = token.trim();
            !token.is_empty() && token.starts_with("eyJ")
        }
        _ => false,
    }
}

/// Parse ddtool output for OAuth URL and device code
fn parse_ddtool_output(output: &str) -> Option<(String, String)> {
    let mut oauth_url = None;
    let mut device_code = None;

    for line in output.lines() {
        // Extract URL after "Open the following link in your browser:"
        if line.contains("https://") && oauth_url.is_none() {
            if let Some(url) = line.split_whitespace().find(|s| s.starts_with("https://")) {
                oauth_url = Some(url.to_string());
            }
        }

        // Extract code after "When prompted, enter code" or similar patterns
        // ddtool typically outputs something like: "Enter code: ABC-DEF-GHI"
        if line.contains("code") && device_code.is_none() {
            // Look for patterns like: ABC-DEF-GHI or ABCDEFGHI
            for word in line.split_whitespace() {
                if word.contains('-') && word.len() >= 7 {
                    // Format: XXX-XXX-XXX
                    let parts: Vec<&str> = word.split('-').collect();
                    if parts.len() >= 2 && parts.iter().all(|p| p.chars().all(|c| c.is_alphanumeric())) {
                        device_code = Some(word.to_string());
                        break;
                    }
                } else if word.len() >= 6 && word.chars().all(|c| c.is_alphanumeric() && c.is_uppercase()) {
                    // Format: ABCDEFGHI
                    device_code = Some(word.to_string());
                    break;
                }
            }
        }
    }

    oauth_url.and_then(|url| device_code.map(|code| (url, code)))
}

/// GET /api/ai-gateway/auth-status
pub async fn get_auth_status(
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Check if AI Gateway is enabled
    if !is_ai_gateway_enabled() {
        return (
            StatusCode::OK,
            Json(AuthStatusResponse {
                status: "not_required".to_string(),
                oauth_url: None,
                device_code: None,
                error: None,
            }),
        );
    }

    let auth_state = state.auth_state.lock().unwrap();
    let response = AuthStatusResponse::from_status(&auth_state.status, &auth_state.process);
    (StatusCode::OK, Json(response))
}

/// POST /api/ai-gateway/initiate-auth
pub async fn initiate_auth(
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Check if AI Gateway is enabled
    if !is_ai_gateway_enabled() {
        return (
            StatusCode::BAD_REQUEST,
            Json(AuthStatusResponse {
                status: "failed".to_string(),
                oauth_url: None,
                device_code: None,
                error: Some("AI Gateway is not enabled".to_string()),
            }),
        );
    }

    // Check if already authenticated (quick check - outputs token immediately)
    if check_auth_exists() {
        let mut auth_state = state.auth_state.lock().unwrap();
        auth_state.status = AuthStatus::Authenticated;
        return (
            StatusCode::OK,
            Json(AuthStatusResponse {
                status: "authenticated".to_string(),
                oauth_url: None,
                device_code: None,
                error: None,
            }),
        );
    }

    let datacenter = std::env::var("AI_GATEWAY_DATACENTER")
        .expect("AI_GATEWAY_DATACENTER must be set when using AI Gateway mode");
    let service_name = std::env::var("AI_GATEWAY_SERVICE")
        .expect("AI_GATEWAY_SERVICE must be set when using AI Gateway mode");

    // Spawn ddtool via script command to provide PTY (ddtool hangs with pipes)
    let ddtool_cmd = format!("ddtool auth token {} --datacenter {}", service_name, datacenter);
    let mut child = match Command::new("script")
        .args(&["-q", "-c", &ddtool_cmd, "/dev/null"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AuthStatusResponse {
                    status: "failed".to_string(),
                    oauth_url: None,
                    device_code: None,
                    error: Some(format!("Failed to spawn ddtool: {}. Is ddtool installed?", e)),
                }),
            );
        }
    };

    // Read initial output to get OAuth URL and device code
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AuthStatusResponse {
                    status: "failed".to_string(),
                    oauth_url: None,
                    device_code: None,
                    error: Some("Failed to capture ddtool stdout".to_string()),
                }),
            );
        }
    };

    // Read stdout with a timeout using a thread
    // ddtool outputs OAuth info quickly, then waits for user - we need to read what we need and stop
    let output = {
        use std::sync::mpsc;
        use std::thread;

        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            use std::io::Read;
            let mut stdout = stdout;
            let mut output = Vec::new();

            // Read with small chunks and timeout - ddtool outputs quickly then waits
            // We just need the first ~1KB which contains OAuth URL and code
            let mut buf = [0u8; 256];
            let start = Instant::now();

            while start.elapsed() < Duration::from_millis(500) && output.len() < 2048 {
                match stdout.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        output.extend_from_slice(&buf[..n]);
                        // If we see "Waiting", we have everything
                        if output.windows(7).any(|w| w == b"Waiting") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
                // Small sleep to avoid busy loop
                std::thread::sleep(Duration::from_millis(10));
            }

            let _ = tx.send(String::from_utf8_lossy(&output).to_string());
        });

        // Wait up to 2 seconds for output
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(out) => out,
            Err(_) => {
                let _ = child.kill();
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(AuthStatusResponse {
                        status: "failed".to_string(),
                        oauth_url: None,
                        device_code: None,
                        error: Some("Timeout reading ddtool output".to_string()),
                    }),
                );
            }
        }
    };

    // Parse OAuth URL and device code
    let (oauth_url, device_code) = match parse_ddtool_output(&output) {
        Some(result) => result,
        None => {
            let _ = child.kill();
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AuthStatusResponse {
                    status: "failed".to_string(),
                    oauth_url: None,
                    device_code: None,
                    error: Some("Failed to parse OAuth URL and device code from ddtool output".to_string()),
                }),
            );
        }
    };

    // Store process state
    let process = DdtoolProcess {
        child,
        oauth_url: oauth_url.clone(),
        device_code: device_code.clone(),
        started_at: Instant::now(),
    };

    let mut auth_state = state.auth_state.lock().unwrap();
    auth_state.status = AuthStatus::InProgress;
    auth_state.process = Some(process);

    (
        StatusCode::OK,
        Json(AuthStatusResponse {
            status: "in_progress".to_string(),
            oauth_url: Some(oauth_url),
            device_code: Some(device_code),
            error: None,
        }),
    )
}

/// GET /api/ai-gateway/poll-auth
pub async fn poll_auth(
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Check if AI Gateway is enabled
    if !is_ai_gateway_enabled() {
        return (
            StatusCode::BAD_REQUEST,
            Json(AuthStatusResponse {
                status: "failed".to_string(),
                oauth_url: None,
                device_code: None,
                error: Some("AI Gateway is not enabled".to_string()),
            }),
        );
    }

    let mut auth_state = state.auth_state.lock().unwrap();

    // Check if process exists
    let process = match auth_state.process.as_mut() {
        Some(p) => p,
        None => {
            // No process, check if auth already exists
            if check_auth_exists() {
                auth_state.status = AuthStatus::Authenticated;
                return (
                    StatusCode::OK,
                    Json(AuthStatusResponse {
                        status: "authenticated".to_string(),
                        oauth_url: None,
                        device_code: None,
                        error: None,
                    }),
                );
            } else {
                auth_state.status = AuthStatus::Required;
                return (
                    StatusCode::OK,
                    Json(AuthStatusResponse {
                        status: "required".to_string(),
                        oauth_url: None,
                        device_code: None,
                        error: None,
                    }),
                );
            }
        }
    };

    // Check timeout
    if process.started_at.elapsed() > AUTH_TIMEOUT {
        let _ = process.child.kill();
        auth_state.status = AuthStatus::Failed("Authentication timed out after 5 minutes".to_string());
        auth_state.process = None;
        return (
            StatusCode::OK,
            Json(AuthStatusResponse {
                status: "failed".to_string(),
                oauth_url: None,
                device_code: None,
                error: Some("Authentication timed out after 5 minutes".to_string()),
            }),
        );
    }

    // Check if process has completed
    match process.child.try_wait() {
        Ok(Some(status)) => {
            // Process completed
            if status.success() {
                auth_state.status = AuthStatus::Authenticated;
                auth_state.process = None;
                (
                    StatusCode::OK,
                    Json(AuthStatusResponse {
                        status: "authenticated".to_string(),
                        oauth_url: None,
                        device_code: None,
                        error: None,
                    }),
                )
            } else {
                auth_state.status = AuthStatus::Failed(format!(
                    "Authentication failed with exit code: {}",
                    status.code().unwrap_or(-1)
                ));
                auth_state.process = None;
                (
                    StatusCode::OK,
                    Json(AuthStatusResponse {
                        status: "failed".to_string(),
                        oauth_url: None,
                        device_code: None,
                        error: Some(format!(
                            "Authentication failed with exit code: {}",
                            status.code().unwrap_or(-1)
                        )),
                    }),
                )
            }
        }
        Ok(None) => {
            // Process still running
            let oauth_url = process.oauth_url.clone();
            let device_code = process.device_code.clone();
            (
                StatusCode::OK,
                Json(AuthStatusResponse {
                    status: "in_progress".to_string(),
                    oauth_url: Some(oauth_url),
                    device_code: Some(device_code),
                    error: None,
                }),
            )
        }
        Err(e) => {
            auth_state.status = AuthStatus::Failed(format!("Failed to check process status: {}", e));
            auth_state.process = None;
            (
                StatusCode::OK,
                Json(AuthStatusResponse {
                    status: "failed".to_string(),
                    oauth_url: None,
                    device_code: None,
                    error: Some(format!("Failed to check process status: {}", e)),
                }),
            )
        }
    }
}

/// Initialize auth state based on environment
pub fn init_auth_state() -> AuthState {
    let mut state = AuthState::new();

    if !is_ai_gateway_enabled() {
        state.status = AuthStatus::NotRequired;
    } else {
        // Check if already authenticated
        // Note: check_auth_exists() now returns false to skip the check during init
        // to avoid blocking server startup. Auth status will be checked lazily.
        state.status = if check_auth_exists() {
            AuthStatus::Authenticated
        } else {
            AuthStatus::Required
        };
    }

    state
}
