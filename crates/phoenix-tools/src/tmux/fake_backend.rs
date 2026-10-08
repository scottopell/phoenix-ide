//! Deterministic in-memory [`TmuxBackend`] for tests. Servers live in a map
//! keyed by socket path, and every operation answers at once, so no test
//! waits on real time or a real process.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use tokio::time::Instant;

use phoenix_core::process_identity::ProcessIdentity;

use super::backend::{
    ExactProcessState, GlobalEnvRead, SocketEndpoint, SocketFileIdentity, TmuxBackend,
    TokenBoundKill, COMPANION_ENV_VERSION, COMPANION_VERSION_VAR, SERVER_TOKEN_VAR,
};
use super::probe::ProbeResult;
use super::registry::TmuxError;

/// Which calls a stalled server never answers. A stalled call that has a
/// deadline returns its deadline result at once. A stalled call without a
/// deadline panics, because against a real server it would hang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stall {
    /// The probe and every server command.
    Everything,
    /// Every server command; the probe still answers.
    AfterProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FakeProcessState {
    Running,
    Zombie,
    Exited,
}

#[derive(Debug, Clone)]
enum FakeEndpoint {
    Socket {
        identity: SocketFileIdentity,
        server: Option<usize>,
    },
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeWindow {
    pub id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub command: String,
    pub pane: String,
}

#[derive(Debug)]
struct FakeServer {
    process: ProcessIdentity,
    env: HashMap<String, String>,
    windows: BTreeMap<String, FakeWindow>,
    cwd: PathBuf,
}

/// A test's view of the server bound to a socket path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeServerView {
    pub token: Option<String>,
    pub process: ProcessIdentity,
    pub live: bool,
    pub cwd: PathBuf,
    pub windows: Vec<FakeWindow>,
}

#[derive(Debug, Default)]
struct FakeWorld {
    endpoints: HashMap<PathBuf, FakeEndpoint>,
    servers: Vec<FakeServer>,
    processes: HashMap<ProcessIdentity, FakeProcessState>,
    stalls: HashMap<PathBuf, Stall>,
    ambiguous: Vec<PathBuf>,
    pane_output: HashMap<String, String>,
    spawns: Vec<PathBuf>,
    kill_server_commands: Vec<PathBuf>,
    next_inode: u64,
    next_pid: u32,
    next_window: u32,
}

impl FakeWorld {
    fn server_id(&self, socket_path: &Path) -> Option<usize> {
        match self.endpoints.get(socket_path) {
            Some(FakeEndpoint::Socket {
                server: Some(id), ..
            }) => Some(*id),
            _ => None,
        }
    }

    fn live_server_id(&self, socket_path: &Path) -> Option<usize> {
        self.server_id(socket_path).filter(|id| {
            self.processes.get(&self.servers[*id].process) == Some(&FakeProcessState::Running)
        })
    }

    fn live_server(&mut self, socket_path: &Path) -> Option<&mut FakeServer> {
        let id = self.live_server_id(socket_path)?;
        Some(&mut self.servers[id])
    }

    fn stall(&self, socket_path: &Path) -> Option<Stall> {
        self.stalls.get(socket_path).copied()
    }

    fn new_socket(&mut self) -> SocketFileIdentity {
        self.next_inode += 1;
        SocketFileIdentity {
            device: 1,
            inode: self.next_inode,
        }
    }

    fn set_process(&mut self, socket_path: &Path, state: FakeProcessState) {
        let id = self
            .server_id(socket_path)
            .unwrap_or_else(|| panic!("no fake tmux server at {}", socket_path.display()));
        let process = self.servers[id].process;
        self.processes.insert(process, state);
    }

    fn run_server_command(&mut self, socket_path: &Path, args: &[String]) -> Output {
        let next_window = self.next_window + 1;
        let new_window_pane = flag_value(args, "-n")
            .and_then(|name| self.pane_output.get(name))
            .cloned()
            .unwrap_or_default();
        let Some(server) = self.live_server(socket_path) else {
            return exit(
                1,
                "",
                format!("no server running on {}\n", socket_path.display()),
            );
        };
        match args.first().map(String::as_str) {
            Some("new-window") => {
                let id = format!("@{next_window}");
                let name = flag_value(args, "-n").unwrap_or(&id).to_string();
                let window = FakeWindow {
                    id: id.clone(),
                    name: name.clone(),
                    cwd: PathBuf::from(flag_value(args, "-c").unwrap_or_default()),
                    command: args.last().cloned().unwrap_or_default(),
                    pane: new_window_pane,
                };
                server.windows.insert(id.clone(), window);
                self.next_window = next_window;
                exit(0, format!("{id}|{name}\n"), "")
            }
            Some("capture-pane") => {
                match flag_value(args, "-t").and_then(|target| server.windows.get(target)) {
                    Some(window) => exit(0, window.pane.clone(), ""),
                    None => exit(1, "", "can't find window\n"),
                }
            }
            Some("kill-window") => {
                match flag_value(args, "-t").and_then(|target| server.windows.remove(target)) {
                    Some(_) => exit(0, "", ""),
                    None => exit(1, "", "can't find window\n"),
                }
            }
            _ => exit(0, "", ""),
        }
    }
}

fn exit(code: i32, stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>) -> Output {
    Output {
        status: ExitStatus::from_raw(code << 8),
        stdout: stdout.into(),
        stderr: stderr.into(),
    }
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn hang(socket_path: &Path) -> ! {
    panic!(
        "call without a deadline against stalled fake tmux server {}",
        socket_path.display()
    )
}

#[derive(Debug)]
pub struct FakeTmuxBackend {
    binary_available: bool,
    world: Mutex<FakeWorld>,
}

impl FakeTmuxBackend {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            binary_available: true,
            world: Mutex::new(FakeWorld {
                next_pid: 1000,
                ..FakeWorld::default()
            }),
        })
    }

    /// A host without the tmux binary.
    #[must_use]
    pub fn unavailable() -> Arc<Self> {
        Arc::new(Self {
            binary_available: false,
            world: Mutex::new(FakeWorld::default()),
        })
    }

    fn world(&self) -> MutexGuard<'_, FakeWorld> {
        self.world.lock().expect("fake tmux world poisoned")
    }

    #[must_use]
    pub fn server(&self, socket_path: &Path) -> Option<FakeServerView> {
        let world = self.world();
        let id = world.server_id(socket_path)?;
        let server = &world.servers[id];
        Some(FakeServerView {
            token: server.env.get(SERVER_TOKEN_VAR).cloned(),
            process: server.process,
            live: world.live_server_id(socket_path).is_some(),
            cwd: server.cwd.clone(),
            windows: server.windows.values().cloned().collect(),
        })
    }

    #[must_use]
    pub fn endpoint_exists(&self, path: &Path) -> bool {
        self.world().endpoints.contains_key(path)
    }

    #[must_use]
    pub fn spawn_count(&self, socket_path: &Path) -> usize {
        self.world()
            .spawns
            .iter()
            .filter(|path| path.as_path() == socket_path)
            .count()
    }

    /// Token-bound `kill-server` commands sent to `socket_path`.
    #[must_use]
    pub fn kill_server_count(&self, socket_path: &Path) -> usize {
        self.world()
            .kill_server_commands
            .iter()
            .filter(|path| path.as_path() == socket_path)
            .count()
    }

    /// The server exits cleanly and tmux unlinks its socket.
    pub fn kill_server(&self, socket_path: &Path) {
        let mut world = self.world();
        world.set_process(socket_path, FakeProcessState::Exited);
        world.endpoints.remove(socket_path);
    }

    /// The server dies and leaves its socket file behind.
    pub fn crash_server(&self, socket_path: &Path) {
        self.world()
            .set_process(socket_path, FakeProcessState::Exited);
    }

    /// The server process is dead but not yet reaped.
    pub fn zombify_server(&self, socket_path: &Path) {
        self.world()
            .set_process(socket_path, FakeProcessState::Zombie);
    }

    pub fn stall(&self, socket_path: &Path, stall: Stall) {
        self.world().stalls.insert(socket_path.to_path_buf(), stall);
    }

    /// `tmux ls` fails without proving that no server runs.
    pub fn make_probe_ambiguous(&self, socket_path: &Path) {
        self.world().ambiguous.push(socket_path.to_path_buf());
    }

    /// A regular file occupies the socket path.
    pub fn place_file(&self, path: &Path) {
        self.world()
            .endpoints
            .insert(path.to_path_buf(), FakeEndpoint::File);
    }

    /// A socket file with no server behind it.
    pub fn place_orphan_socket(&self, path: &Path) {
        let mut world = self.world();
        let identity = world.new_socket();
        world.endpoints.insert(
            path.to_path_buf(),
            FakeEndpoint::Socket {
                identity,
                server: None,
            },
        );
    }

    pub fn remove_global_env(&self, socket_path: &Path, var: &str) {
        if let Some(server) = self.world().live_server(socket_path) {
            server.env.remove(var);
        }
    }

    /// Pane text that `capture-pane` returns for a window opened with
    /// `new-window -n <window_name>`.
    pub fn set_pane_output(&self, window_name: &str, pane: &str) {
        self.world()
            .pane_output
            .insert(window_name.to_string(), pane.to_string());
    }

    /// Open a window on a live server and return its id.
    ///
    /// # Panics
    /// Panics when no live server owns `socket_path`.
    pub fn add_window(&self, socket_path: &Path, pane: &str) -> String {
        let mut world = self.world();
        world.next_window += 1;
        let id = format!("@{}", world.next_window);
        let server = world
            .live_server(socket_path)
            .unwrap_or_else(|| panic!("no live fake tmux server at {}", socket_path.display()));
        let cwd = server.cwd.clone();
        server.windows.insert(
            id.clone(),
            FakeWindow {
                id: id.clone(),
                name: id.clone(),
                cwd,
                command: String::new(),
                pane: pane.to_string(),
            },
        );
        id
    }
}

#[async_trait]
impl TmuxBackend for FakeTmuxBackend {
    fn binary_available(&self) -> bool {
        self.binary_available
    }

    async fn probe(
        &self,
        socket_path: &Path,
        expires: Option<Instant>,
    ) -> std::io::Result<Option<ProbeResult>> {
        let world = self.world();
        if world.stall(socket_path) == Some(Stall::Everything) {
            return match expires {
                Some(_) => Ok(None),
                None => hang(socket_path),
            };
        }
        let result = match world.endpoints.get(socket_path) {
            None => ProbeResult::NoSocket,
            Some(FakeEndpoint::File) => ProbeResult::DeadSocket,
            Some(FakeEndpoint::Socket { .. })
                if world.ambiguous.iter().any(|p| p == socket_path) =>
            {
                ProbeResult::DeadSocket
            }
            Some(FakeEndpoint::Socket { .. }) if world.live_server_id(socket_path).is_some() => {
                ProbeResult::Live
            }
            Some(FakeEndpoint::Socket { .. }) => ProbeResult::NoServer,
        };
        Ok(Some(result))
    }

    fn endpoint(&self, path: &Path) -> std::io::Result<SocketEndpoint> {
        Ok(match self.world().endpoints.get(path) {
            None => SocketEndpoint::Absent,
            Some(FakeEndpoint::File) => SocketEndpoint::NotSocket,
            Some(FakeEndpoint::Socket { identity, .. }) => SocketEndpoint::Socket(*identity),
        })
    }

    async fn remove_endpoint(&self, path: &Path) -> std::io::Result<()> {
        match self.world().endpoints.remove(path) {
            Some(_) => Ok(()),
            None => Err(std::io::ErrorKind::NotFound.into()),
        }
    }

    async fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut paths = self
            .world()
            .endpoints
            .keys()
            .filter(|path| path.parent() == Some(dir))
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }

    async fn spawn_session(
        &self,
        socket_path: &Path,
        _config_path: &Path,
        cwd: &Path,
    ) -> Result<(), TmuxError> {
        let mut world = self.world();
        world.next_pid += 1;
        let process = ProcessIdentity {
            pid: world.next_pid,
            start_time: u128::from(world.next_pid),
        };
        let env = HashMap::from([
            (
                SERVER_TOKEN_VAR.to_string(),
                uuid::Uuid::new_v4().to_string(),
            ),
            (
                COMPANION_VERSION_VAR.to_string(),
                COMPANION_ENV_VERSION.to_string(),
            ),
        ]);
        world.servers.push(FakeServer {
            process,
            env,
            windows: BTreeMap::new(),
            cwd: cwd.to_path_buf(),
        });
        let server = Some(world.servers.len() - 1);
        world.processes.insert(process, FakeProcessState::Running);
        let identity = world.new_socket();
        world.endpoints.insert(
            socket_path.to_path_buf(),
            FakeEndpoint::Socket { identity, server },
        );
        world.spawns.push(socket_path.to_path_buf());
        Ok(())
    }

    async fn global_env(
        &self,
        socket_path: &Path,
        var: &str,
        expires: Option<Instant>,
    ) -> GlobalEnvRead {
        let mut world = self.world();
        if world.stall(socket_path).is_some() {
            return match expires {
                Some(_) => GlobalEnvRead::DeadlineExceeded,
                None => hang(socket_path),
            };
        }
        world
            .live_server(socket_path)
            .and_then(|server| server.env.get(var).cloned())
            .map_or(GlobalEnvRead::Unreadable, GlobalEnvRead::Value)
    }

    async fn set_global_env(&self, socket_path: &Path, var: &str, value: &str) {
        if let Some(server) = self.world().live_server(socket_path) {
            server.env.insert(var.to_string(), value.to_string());
        }
    }

    async fn refresh_companion_if_stale(&self, socket_path: &Path) {
        self.set_global_env(socket_path, COMPANION_VERSION_VAR, COMPANION_ENV_VERSION)
            .await;
    }

    async fn server_process(
        &self,
        socket_path: &Path,
        expected_token: &str,
        _expires: Instant,
    ) -> Option<ProcessIdentity> {
        let mut world = self.world();
        if world.stall(socket_path).is_some() {
            return None;
        }
        let server = world.live_server(socket_path)?;
        (server.env.get(SERVER_TOKEN_VAR).map(String::as_str) == Some(expected_token))
            .then_some(server.process)
    }

    async fn kill_server_if_token(
        &self,
        socket_path: &Path,
        _config_path: &Path,
        expected_token: &str,
        _expires: Instant,
    ) -> TokenBoundKill {
        let mut world = self.world();
        if world.stall(socket_path).is_some() {
            return TokenBoundKill::DeadlineExceeded;
        }
        let Some(server) = world.live_server(socket_path) else {
            return TokenBoundKill::Failed {
                reason: format!(
                    "exact token-bound kill-server failed: no server running on {}",
                    socket_path.display()
                ),
            };
        };
        if server.env.get(SERVER_TOKEN_VAR).map(String::as_str) != Some(expected_token) {
            return TokenBoundKill::TokenMismatch;
        }
        let process = server.process;
        world.processes.insert(process, FakeProcessState::Exited);
        world.endpoints.remove(socket_path);
        world.kill_server_commands.push(socket_path.to_path_buf());
        TokenBoundKill::Killed
    }

    async fn capture_pane(
        &self,
        socket_path: &Path,
        window_id: &str,
    ) -> std::io::Result<Option<String>> {
        Ok(self
            .world()
            .live_server(socket_path)
            .and_then(|server| server.windows.get(window_id))
            .map(|window| window.pane.clone()))
    }

    async fn kill_window(&self, socket_path: &Path, window_id: &str) {
        if let Some(server) = self.world().live_server(socket_path) {
            server.windows.remove(window_id);
        }
    }

    fn process_state(&self, process: ProcessIdentity) -> ExactProcessState {
        match self.world().processes.get(&process) {
            Some(FakeProcessState::Running) => ExactProcessState::Live,
            Some(FakeProcessState::Zombie | FakeProcessState::Exited) => {
                ExactProcessState::DeadOrReused
            }
            None => ExactProcessState::Unproven,
        }
    }

    async fn wait_process_exit(
        &self,
        process: ProcessIdentity,
        _expires: Instant,
    ) -> ExactProcessState {
        self.process_state(process)
    }

    async fn run_cli(
        &self,
        _config_path: &Path,
        socket_path: &Path,
        args: &[String],
    ) -> Result<Output, String> {
        Ok(self.world().run_server_command(socket_path, args))
    }
}
