use super::daemon;
use super::protocol::{self, Outcome, Request, Response, SessionPaths};
use fs2::FileExt;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::UnixStream;

const HELP: &str = "phoenix-browser — persistent browser for coding agents (macOS/Linux)

Usage: phoenix-browser [--session NAME] [--json] [--idle-timeout SECONDS] COMMAND

  open URL                         Navigate; lazily start a browser
  snapshot [--interactive]          Accessibility tree with click/type refs
  click TARGET                     Click @ref or CSS selector
  type TARGET TEXT [--clear]        Type into @ref or CSS selector
  eval EXPRESSION | eval --stdin    Evaluate JavaScript (await promises)
  screenshot [--selector CSS]       Save a PNG artifact
  resize WIDTH HEIGHT              Set viewport dimensions
  key KEY [--modifiers JSON]        Key or chord (e.g. key Enter)
  wait SELECTOR                    Wait for a CSS selector
  console [--limit N]               Read captured console entries
  console-clear                    Clear captured console entries
  profile ACTION [--input JSON]     Profiling; profile help lists actions
  status                           Show browser lifetime and daemon config
  close                            Stop browser and daemon, discard profile
  schema [COMMAND]                 Show operation input schemas

Every operation accepts --input JSON for its full Phoenix parameter surface.
Use --timeout DURATION for browser operation budgets (e.g. 15s, 500ms).
Session: --session > PHOENIX_BROWSER_SESSION > Git worktree root (else cwd).
The fallback shares one browser within a worktree. Bind a unique session per agent.
Idle timeout: --idle-timeout > PHOENIX_BROWSER_IDLE_TIMEOUT > 1800 seconds;
chosen when a daemon starts. Active requests do not count as idle.
Errors: exit 1; invalid CLI input: exit 2. --json emits a tagged success/error.
Only open starts a browser; after state loss, inspect with status then open again.
See docs/phoenix-browser.md for profiling and lifecycle examples.";

pub struct Options {
    pub session: String,
    pub paths: SessionPaths,
    pub idle: Duration,
    pub json: bool,
    pub request: Request,
}

fn take_option(args: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    let mut found = None;
    while let Some(index) = args.iter().position(|arg| arg == name) {
        args.remove(index);
        if index == args.len() || args[index].starts_with("--") {
            return Err(format!("{name} requires a value"));
        }
        if found.is_some() {
            return Err(format!("{name} may only be specified once"));
        }
        found = Some(args.remove(index));
    }
    Ok(found)
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    let before = args.len();
    args.retain(|arg| arg != name);
    args.len() != before
}

pub fn resolve_session(
    explicit: Option<String>,
    environment: Option<String>,
    cwd: &Path,
) -> Result<String, String> {
    if let Some(name) = explicit.or(environment) {
        if name.trim().is_empty() {
            return Err("session name must not be empty".into());
        }
        return Ok(format!("named:{name}"));
    }
    let root = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|root| PathBuf::from(root.strip_suffix('\n').unwrap_or(&root)));
    let path = root
        .as_deref()
        .unwrap_or(cwd)
        .canonicalize()
        .map_err(|e| format!("resolve session directory: {e}"))?;
    Ok(format!("directory:{}", path.display()))
}

pub fn parse_idle(value: &str) -> Result<Duration, String> {
    let seconds: u64 = value
        .parse()
        .map_err(|_| "idle timeout must be a positive integer number of seconds")?;
    if seconds == 0 || seconds > 604_800 {
        return Err("idle timeout must be 1..604800 seconds".into());
    }
    Ok(Duration::from_secs(seconds))
}

fn target(value: &str) -> Value {
    if value.starts_with('@') {
        json!({"ref":value})
    } else {
        json!({"selector":value})
    }
}

fn parse_request(mut args: Vec<String>) -> Result<Request, String> {
    if args.is_empty() {
        return Err("missing command; use --help".into());
    }
    let command = args.remove(0);
    let mut input = match take_option(&mut args, "--input")? {
        Some(value) => serde_json::from_str::<Value>(&value)
            .map_err(|e| format!("invalid --input JSON: {e}"))?,
        None => json!({}),
    };
    if !input.is_object() {
        return Err("--input must be a JSON object".into());
    }
    if let Some(timeout) = take_option(&mut args, "--timeout")? {
        input["timeout"] = json!(timeout);
    }
    let interactive = take_flag(&mut args, "--interactive");
    let clear = take_flag(&mut args, "--clear");
    let stdin = take_flag(&mut args, "--stdin");
    if interactive {
        input["interactive"] = json!(true);
    }
    if clear {
        input["clear"] = json!(true);
    }
    if let Some(selector) = take_option(&mut args, "--selector")? {
        input["selector"] = json!(selector);
    }
    if let Some(limit) = take_option(&mut args, "--limit")? {
        input["limit"] = json!(limit
            .parse::<u64>()
            .map_err(|_| "--limit must be an integer")?);
    }
    if let Some(modifiers) = take_option(&mut args, "--modifiers")? {
        input["modifiers"] =
            serde_json::from_str(&modifiers).map_err(|e| format!("invalid modifiers JSON: {e}"))?;
    }
    positional(&command, &args, &mut input, stdin)?;
    validate_parameters(&command, &input)?;
    for field in ["path", "baseline"] {
        if let Some(path) = input.get(field).and_then(Value::as_str) {
            let path = PathBuf::from(path);
            if path.is_relative() {
                input[field] = json!(std::env::current_dir()
                    .map_err(|e| e.to_string())?
                    .join(path));
            }
        }
    }
    Ok(Request { command, input })
}

fn validate_parameters(command: &str, input: &Value) -> Result<(), String> {
    let Some(operation) = daemon::operation(command) else {
        return Ok(());
    };
    let schema = operation.input_schema();
    if let Some(required) = schema["required"].as_array() {
        for key in required.iter().filter_map(Value::as_str) {
            if input.get(key).is_none() {
                return Err(format!("{command} requires {key}; use schema {command}"));
            }
        }
    }
    if let (Some(properties), Some(parameters)) =
        (schema["properties"].as_object(), input.as_object())
    {
        for key in parameters.keys() {
            if !properties.contains_key(key) {
                return Err(format!(
                    "unsupported {command} parameter {key:?}; use schema {command}"
                ));
            }
        }
    }
    Ok(())
}

fn positional(
    command: &str,
    args: &[String],
    input: &mut Value,
    stdin: bool,
) -> Result<(), String> {
    let expected = match command {
        "open" => {
            if let Some(url) = args.first() {
                input["url"] = json!(url);
            }
            usize::from(!args.is_empty())
        }
        "eval" if stdin => {
            if !args.is_empty() {
                return Err("eval --stdin does not accept an expression argument".into());
            }
            let mut expression = String::new();
            std::io::stdin()
                .take(protocol::MAX_MESSAGE)
                .read_to_string(&mut expression)
                .map_err(|e| e.to_string())?;
            input["expression"] = json!(expression);
            0
        }
        "eval" => {
            if let Some(expression) = args.first() {
                input["expression"] = json!(expression);
            }
            usize::from(!args.is_empty())
        }
        "click" | "type" => {
            if let Some(selector) = args.first() {
                input
                    .as_object_mut()
                    .unwrap()
                    .extend(target(selector).as_object().unwrap().clone());
            }
            if command == "type" {
                if let Some(text) = args.get(1) {
                    input["text"] = json!(text);
                }
                args.len().min(2)
            } else {
                usize::from(!args.is_empty())
            }
        }
        "resize" => {
            if let Some(width) = args.first() {
                input["width"] = json!(width
                    .parse::<u32>()
                    .map_err(|_| "width must be an integer")?);
            }
            if let Some(height) = args.get(1) {
                input["height"] = json!(height
                    .parse::<u32>()
                    .map_err(|_| "height must be an integer")?);
            }
            args.len().min(2)
        }
        "key" => {
            if let Some(key) = args.first() {
                input["key"] = json!(key);
            }
            usize::from(!args.is_empty())
        }
        "wait" => {
            if let Some(selector) = args.first() {
                input["selector"] = json!(selector);
            }
            usize::from(!args.is_empty())
        }
        "profile" => {
            if let Some(action) = args.first() {
                input["action"] = json!(action);
            }
            usize::from(!args.is_empty())
        }
        "schema" => {
            if let Some(operation) = args.first() {
                input["command"] = json!(operation);
            }
            usize::from(!args.is_empty())
        }
        "snapshot" | "screenshot" | "console" | "console-clear" | "status" | "close" => 0,
        _ => return Err(format!("unknown command {command:?}; use --help")),
    };
    if args.len() != expected {
        return Err(format!(
            "unexpected arguments for {command}: {:?}",
            &args[expected..]
        ));
    }
    if stdin && command != "eval" {
        return Err("--stdin is only valid with eval".into());
    }
    Ok(())
}

fn options(mut args: Vec<String>) -> Result<Options, String> {
    let json = take_flag(&mut args, "--json");
    let explicit = take_option(&mut args, "--session")?;
    let idle = take_option(&mut args, "--idle-timeout")?
        .or_else(|| std::env::var("PHOENIX_BROWSER_IDLE_TIMEOUT").ok())
        .unwrap_or_else(|| "1800".into());
    let session = resolve_session(
        explicit,
        std::env::var("PHOENIX_BROWSER_SESSION").ok(),
        &std::env::current_dir().map_err(|e| e.to_string())?,
    )?;
    let key = hex::encode(&Sha256::digest(session.as_bytes())[..12]);
    let directory = std::env::var_os("PHOENIX_BROWSER_RUNTIME_DIR").map_or_else(
        || {
            PhoenixRuntimeEnvironment::detect()
                .data_dir()
                .join("browser-cli")
        },
        PathBuf::from,
    );
    let paths = SessionPaths { directory, key };
    let request = parse_request(args)?;
    Ok(Options {
        session,
        paths,
        idle: parse_idle(&idle)?,
        json,
        request,
    })
}

async fn startup_lock(
    paths: &SessionPaths,
    deadline: tokio::time::Instant,
) -> Result<std::fs::File, String> {
    let lock = daemon::lock_file(&paths.startup_lock())?;
    loop {
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => return Ok(lock),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("startup lock: {error}")),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("timed out waiting for session startup".into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

enum ConnectionError {
    StateLost,
    Startup(String),
}
impl From<String> for ConnectionError {
    fn from(message: String) -> Self {
        Self::Startup(message)
    }
}

async fn connect(paths: &SessionPaths) -> Result<Option<UnixStream>, String> {
    match UnixStream::connect(paths.socket()).await {
        Ok(stream) => Ok(Some(stream)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("connect session endpoint: {error}")),
    }
}

async fn connect_or_start(options: &Options) -> Result<UnixStream, ConnectionError> {
    if options.paths.directory.exists() {
        daemon::private_directory(&options.paths.directory)?;
    }
    if let Some(stream) = connect(&options.paths).await? {
        return Ok(stream);
    }
    if options.request.command != "open" {
        return Err(ConnectionError::StateLost);
    }
    daemon::private_directory(&options.paths.directory)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let _startup = startup_lock(&options.paths, deadline).await?;
    let owner = daemon::lock_file(&options.paths.owner_lock())?;
    loop {
        if let Some(stream) = connect(&options.paths).await? {
            return Ok(stream);
        }
        match FileExt::try_lock_exclusive(&owner) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("owner lock: {error}").into()),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ConnectionError::Startup("session owner is shutting down or unavailable; inspect status before starting again".into()));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    drop(owner);
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = daemon::append_log(&options.paths.log())?;
    let mut command = std::process::Command::new(executable);
    command
        .args([
            "__daemon",
            &options.paths.key,
            &options.session,
            &options.idle.as_secs().to_string(),
        ])
        .arg(&options.paths.directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("daemon startup: {e}"))?;
    loop {
        if let Some(stream) = connect(&options.paths).await? {
            return Ok(stream);
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "daemon exited {status}; inspect {}",
                options.paths.log().display()
            )
            .into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "daemon startup timed out; inspect {}",
                options.paths.log().display()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn execute(options: &Options) -> Response {
    if options.request.command == "schema"
        || (options.request.command == "profile"
            && matches!(
                options.request.input["action"].as_str(),
                Some("help" | "cpu_summary")
            ))
    {
        return daemon::offline(&options.session, &options.request).await;
    }
    let mut stream = match connect_or_start(options).await {
        Ok(stream) => stream,
        Err(ConnectionError::StateLost)
            if matches!(options.request.command.as_str(), "status" | "close") =>
        {
            return Response::success(&options.session, None, json!({"active":false}));
        }
        Err(ConnectionError::StateLost) => {
            return Response::error(
                &options.session,
                None,
                "state_lost",
                "No live browser for this session; use open URL to start fresh",
            )
        }
        Err(ConnectionError::Startup(error)) => {
            return Response::error(&options.session, None, "startup_failed", error)
        }
    };
    if let Err(error) = protocol::write_message(&mut stream, &options.request).await {
        return Response::error(
            &options.session,
            None,
            "transport_uncertain",
            format!("request may have run; not replayed: {error}"),
        );
    }
    match protocol::read_message(&mut stream).await {
        Ok(response) => response,
        Err(error) => Response::error(
            &options.session,
            None,
            "transport_uncertain",
            format!("request may have run; not replayed: {error}"),
        ),
    }
}

fn print_response(response: &Response, json_mode: bool) {
    if json_mode {
        println!(
            "{}",
            serde_json::to_string(response).expect("response serializes")
        );
        return;
    }
    match &response.outcome {
        Outcome::Success { result } if result.get("navigation").is_some() => {
            println!(
                "{} — {}",
                result["title"].as_str().unwrap_or("(untitled)"),
                result["url"].as_str().unwrap_or("(URL unavailable)")
            );
            if result["snapshot"]["status"] == "success" {
                if let Some(text) = result["snapshot"]["result"]["text"].as_str() {
                    println!("{text}");
                }
            } else {
                eprintln!(
                    "Snapshot failed: {}",
                    result["snapshot"]["error"]["message"]
                );
            }
            if let Some(errors) = result["console_errors"].as_array() {
                for error in errors {
                    eprintln!(
                        "Console {}: {}",
                        error["level"].as_str().unwrap_or("error"),
                        error["text"].as_str().unwrap_or("")
                    );
                }
            }
        }
        Outcome::Success { result } => {
            if let Some(text) = result.get("text").and_then(Value::as_str) {
                println!("{text}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(result).expect("result serializes")
                );
            }
        }
        Outcome::Error { error } => eprintln!("{}: {}", error.code, error.message),
    }
}

pub async fn main_entry() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "__daemon") {
        return match daemon::run_from_args(&args[1..]).await {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
    }
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{HELP}");
        return 0;
    }
    if args == ["--version"] {
        println!("phoenix-browser {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let json_mode = args.iter().any(|arg| arg == "--json");
    let options = match options(args) {
        Ok(options) => options,
        Err(error) => {
            print_response(
                &Response::error("unresolved", None, "invalid_input", error),
                json_mode,
            );
            return 2;
        }
    };
    let response = execute(&options).await;
    print_response(&response, options.json);
    i32::from(!response.is_success())
}
