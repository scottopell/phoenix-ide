#![cfg(target_os = "macos")]

use phoenix_deployment_controller::{
    AttemptId, BirthToken, ControllerSqliteStore, MonotonicTick, ProcessId, ProcessIdentity,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem::{size_of, MaybeUninit},
    num::NonZeroU32,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const LABEL_NAMESPACE: &str = "test.disposable.sdc.guardian.";
pub(super) const ATTEMPT: AttemptId = AttemptId::new(64);
const WAIT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: u64 = 128 * 1024;

enum GuardianProgram {
    SameBuild,
    StagedInRoot,
}

struct JobSpec {
    root: tempfile::TempDir,
    label: String,
    uid: u32,
    deadline: MonotonicTick,
    child_seconds: u64,
    serve_seconds: u64,
    guardian_program: GuardianProgram,
}

impl JobSpec {
    fn new() -> io::Result<Self> {
        Self::new_with_timing(110, 90, 120)
    }

    fn new_with_timing(
        child_seconds: u64,
        deadline_seconds: u64,
        serve_seconds: u64,
    ) -> io::Result<Self> {
        let root = tempfile::Builder::new()
            .prefix("sdc-disposable-")
            .tempdir_in("/tmp")?;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
        // SAFETY: these calls have no pointer arguments or side effects.
        let uid = unsafe { libc::getuid() };
        if uid != unsafe { libc::geteuid() } {
            return Err(io::Error::other("test must not run setuid"));
        }
        let suffix = root.path().file_name().unwrap().to_str().unwrap();
        let label = format!("{LABEL_NAMESPACE}{}.{suffix}", std::process::id());
        let deadline = MonotonicTick::native_now()?
            .checked_add(Duration::from_secs(deadline_seconds))
            .ok_or_else(|| io::Error::other("deadline overflow"))?;
        Ok(Self {
            root,
            label,
            uid,
            deadline,
            child_seconds,
            serve_seconds,
            guardian_program: GuardianProgram::SameBuild,
        })
    }

    fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }
    fn target(&self) -> String {
        format!("{}/{}", self.domain(), self.label)
    }
    fn socket_dir(&self) -> PathBuf {
        self.root.path().join("sdc-disposable-ipc")
    }
    fn store(&self) -> PathBuf {
        self.root.path().join("controller.sqlite")
    }
    fn plist_path(&self) -> PathBuf {
        self.root.path().join("guardian.plist")
    }
    fn stdout(&self) -> PathBuf {
        self.root.path().join("guardian.stdout")
    }
    fn stderr(&self) -> PathBuf {
        self.root.path().join("guardian.stderr")
    }

    fn arguments(&self) -> Vec<String> {
        vec![
            match self.guardian_program {
                GuardianProgram::StagedInRoot => self
                    .root
                    .path()
                    .join("bin/guardian-test")
                    .to_str()
                    .unwrap()
                    .to_owned(),
                GuardianProgram::SameBuild => {
                    env!("CARGO_BIN_EXE_phoenix-deployment-guardian").to_owned()
                }
            },
            "--socket-dir".into(),
            self.socket_dir().to_str().unwrap().into(),
            "--child-seconds".into(),
            self.child_seconds.to_string(),
            "--serve-seconds".into(),
            self.serve_seconds.to_string(),
            "--attempt".into(),
            ATTEMPT.get().to_string(),
            "--deadline-nanoseconds".into(),
            self.deadline.get().to_string(),
            "--store".into(),
            self.store().to_str().unwrap().into(),
        ]
    }

    fn plist(&self) -> String {
        let args = plist_arguments(&self.arguments());
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>{}</string>\n\
             <key>ProgramArguments</key><array>{args}</array>\n\
             <key>RunAtLoad</key><true/>\n<key>KeepAlive</key><false/>\n\
             <key>ProcessType</key><string>Background</string>\n\
             <key>Umask</key><integer>63</integer>\n\
             <key>StandardOutPath</key><string>{}</string>\n\
             <key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            xml(&self.label),
            xml(self.stdout().to_str().unwrap()),
            xml(self.stderr().to_str().unwrap()),
        )
    }
}

fn plist_arguments(arguments: &[String]) -> String {
    let mut result = String::new();
    for argument in arguments {
        result.push_str("<string>");
        result.push_str(&xml(argument));
        result.push_str("</string>");
    }
    result
}

#[derive(Clone, Copy)]
enum LaunchctlAction {
    Bootstrap,
    Print,
    Bootout,
}

impl LaunchctlAction {
    fn arguments(self, spec: &JobSpec) -> Vec<String> {
        match self {
            Self::Bootstrap => vec![
                "bootstrap".into(),
                spec.domain(),
                spec.plist_path().to_str().unwrap().into(),
            ],
            Self::Print => vec!["print".into(), spec.target()],
            Self::Bootout => vec!["bootout".into(), spec.target()],
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct CleanupFailures(Arc<Mutex<Vec<String>>>);

impl CleanupFailures {
    pub(super) fn record(&self, message: impl Into<String>) {
        self.0.lock().unwrap().push(message.into());
    }

    fn summary(&self) -> Option<String> {
        let failures = self.0.lock().unwrap();
        (!failures.is_empty()).then(|| failures.join("; "))
    }
}

struct LaunchctlProcess {
    child: Child,
    cleanup_failures: CleanupFailures,
}

impl Drop for LaunchctlProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        self.child.kill().ok();
        let until = Instant::now() + WAIT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < until => poll_pause(),
                result => {
                    self.cleanup_failures.record(format!(
                        "disposable launchctl child cleanup unverified: {result:?}"
                    ));
                    return;
                }
            }
        }
    }
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

pub(super) fn bounded_text(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_OUTPUT {
        return Err(io::Error::other("fixture output limit exceeded"));
    }
    let mut text = String::new();
    file.take(MAX_OUTPUT + 1).read_to_string(&mut text)?;
    if u64::try_from(text.len()).unwrap() > MAX_OUTPUT {
        return Err(io::Error::other("fixture output limit exceeded"));
    }
    Ok(text)
}

pub(super) fn poll_pause() {
    // test-timing-allow: all callers have explicit hard deadlines for causal OS/file observations.
    thread::sleep(Duration::from_millis(20));
}

#[derive(Clone, Copy, Debug)]
struct LiveProcess {
    identity: ProcessIdentity,
    uid: u32,
    parent: u32,
    group: u32,
}

fn inspect(pid: u32) -> io::Result<LiveProcess> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = i32::try_from(size_of::<libc::proc_bsdinfo>()).unwrap();
    // SAFETY: the buffer is writable for exactly the requested native struct size.
    let n = unsafe {
        libc::proc_pidinfo(
            i32::try_from(pid).map_err(|_| io::Error::other("PID out of native range"))?,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n == 0 {
        return Err(io::Error::last_os_error());
    }
    if n != size {
        return Err(io::Error::other("short process observation"));
    }
    // SAFETY: the kernel filled the whole struct above.
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid {
        return Err(io::Error::other("changed process observation"));
    }
    let nanos = u32::try_from(info.pbi_start_tvusec)
        .ok()
        .and_then(|v| v.checked_mul(1000))
        .ok_or_else(|| io::Error::other("invalid birth"))?;
    let birth = BirthToken::new(info.pbi_start_tvsec, nanos)
        .map_err(|_| io::Error::other("invalid birth token"))?;
    Ok(LiveProcess {
        identity: ProcessIdentity::new(
            ProcessId::new(NonZeroU32::new(pid).ok_or_else(|| io::Error::other("zero pid"))?),
            birth,
        ),
        uid: info.pbi_uid,
        parent: info.pbi_ppid,
        group: info.pbi_pgid,
    })
}

fn child_output(text: &str) -> Option<ProcessIdentity> {
    let line = text
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .find_map(|line| line.trim_end().strip_prefix("disposable child pid="))?;
    let (pid, birth) = line.split_once(" birth=")?;
    let (seconds, nanos) = birth.split_once(':')?;
    let pid = native_pid(pid)?;
    Some(ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(pid)?),
        BirthToken::new(seconds.parse().ok()?, nanos.parse().ok()?).ok()?,
    ))
}

fn native_pid(text: &str) -> Option<u32> {
    let pid: u32 = text.parse().ok()?;
    (pid > 1 && i32::try_from(pid).is_ok()).then_some(pid)
}

fn job_pid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| native_pid(line.trim().strip_prefix("pid = ")?))
}

fn private_path(path: &Path, uid: u32) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other("nonprivate fixture path"));
    }
    Ok(meta)
}

#[derive(Debug, Eq, PartialEq)]
enum JobObservation {
    Absent,
    Loaded { pid: Option<u32> },
}

impl JobObservation {
    fn from_print(success: bool, stdout: &str, stderr: &str, label: &str) -> io::Result<Self> {
        if success {
            return Ok(Self::Loaded {
                pid: job_pid(stdout),
            });
        }
        if stderr.contains("Could not find service") && stderr.contains(&format!("\"{label}\"")) {
            return Ok(Self::Absent);
        }
        Err(io::Error::other(format!(
            "cannot verify exact job: {stderr}"
        )))
    }
}

// No Clone, caller-selected label/program, PID-based constructor, or production adapter.
pub(super) struct DisposableLaunchdGuardian {
    spec: JobSpec,
    armed: bool,
    guardian: Option<ProcessIdentity>,
    child: Option<ProcessIdentity>,
    ctl_sequence: u32,
    cleanup_failures: CleanupFailures,
}

impl DisposableLaunchdGuardian {
    pub(super) fn start() -> io::Result<Self> {
        Self::start_spec(JobSpec::new()?)
    }

    fn start_spec(spec: JobSpec) -> io::Result<Self> {
        drop(ControllerSqliteStore::open(spec.store()).map_err(io::Error::other)?);
        fs::set_permissions(spec.store(), fs::Permissions::from_mode(0o600))?;
        private_file(&spec.plist_path())?.write_all(spec.plist().as_bytes())?;
        drop(private_file(&spec.stdout())?);
        drop(private_file(&spec.stderr())?);
        let mut owned = Self {
            spec,
            armed: true,
            guardian: None,
            child: None,
            ctl_sequence: 0,
            cleanup_failures: CleanupFailures::default(),
        };
        let domain = owned.spec.domain();
        let (status, _, stderr) = owned.launchctl(LaunchctlAction::Bootstrap)?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "disposable bootstrap failed in {domain}; requires this UID's logged-in GUI domain (no system/user fallback, sudo, or silent skip): {stderr}"
            )));
        }
        owned.wait_ready()?;
        Ok(owned)
    }

    pub(super) fn root(&self) -> &Path {
        self.spec.root.path()
    }
    pub(super) fn socket_dir(&self) -> PathBuf {
        self.spec.socket_dir()
    }
    pub(super) fn store(&self) -> PathBuf {
        self.spec.store()
    }
    pub(super) fn deadline(&self) -> MonotonicTick {
        self.spec.deadline
    }
    pub(super) fn guardian(&self) -> ProcessIdentity {
        self.guardian.unwrap()
    }
    pub(super) fn child(&self) -> ProcessIdentity {
        self.child.unwrap()
    }

    pub(super) fn cleanup_failures(&self) -> CleanupFailures {
        self.cleanup_failures.clone()
    }

    fn launchctl(&mut self, action: LaunchctlAction) -> io::Result<(ExitStatus, String, String)> {
        self.ctl_sequence += 1;
        let out = self
            .root()
            .join(format!("ctl-{}.stdout", self.ctl_sequence));
        let err = self
            .root()
            .join(format!("ctl-{}.stderr", self.ctl_sequence));
        let mut process = LaunchctlProcess {
            child: Command::new("/bin/launchctl")
                .args(action.arguments(&self.spec))
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(private_file(&out)?)
                .stderr(private_file(&err)?)
                .spawn()?,
            cleanup_failures: self.cleanup_failures.clone(),
        };
        let until = Instant::now() + WAIT;
        let status = loop {
            if let Some(status) = process.child.try_wait()? {
                break status;
            }
            if Instant::now() >= until {
                return Err(io::Error::other("bounded launchctl timed out"));
            }
            poll_pause();
        };
        Ok((status, bounded_text(&out)?, bounded_text(&err)?))
    }

    fn job(&mut self) -> io::Result<JobObservation> {
        let (status, stdout, stderr) = self.launchctl(LaunchctlAction::Print)?;
        JobObservation::from_print(status.success(), &stdout, &stderr, &self.spec.label)
    }

    pub(super) fn job_pid(&mut self) -> io::Result<Option<u32>> {
        match self.job()? {
            JobObservation::Loaded { pid } => Ok(pid),
            JobObservation::Absent => Ok(None),
        }
    }

    fn wait_ready(&mut self) -> io::Result<()> {
        let until = Instant::now() + WAIT;
        loop {
            private_path(self.root(), self.spec.uid)?;
            if let Some(pid) = self.job_pid()? {
                let guardian = inspect(pid)?;
                if guardian.uid != self.spec.uid {
                    return Err(io::Error::other("wrong guardian UID"));
                }
                if let Some(original) = self.guardian {
                    if original != guardian.identity {
                        return Err(io::Error::other("guardian replaced"));
                    }
                }
                self.guardian = Some(guardian.identity);
                if let Some(child) = child_output(&bounded_text(&self.spec.stdout())?) {
                    self.child = Some(child);
                    let socket = self.socket_dir().join("guardian.sock");
                    if socket.exists() {
                        if !private_path(&self.socket_dir(), self.spec.uid)?.is_dir()
                            || !private_path(&socket, self.spec.uid)?
                                .file_type()
                                .is_socket()
                        {
                            return Err(io::Error::other("invalid socket shape"));
                        }
                        for path in [
                            self.spec.plist_path(),
                            self.spec.stdout(),
                            self.spec.stderr(),
                            self.store(),
                        ] {
                            if !private_path(&path, self.spec.uid)?.is_file() {
                                return Err(io::Error::other("invalid private file"));
                            }
                        }
                        return self.assert_original_parent();
                    }
                }
            }
            if Instant::now() >= until {
                return Err(io::Error::other(format!(
                    "guardian readiness timed out: {}",
                    bounded_text(&self.spec.stderr())?
                )));
            }
            poll_pause();
        }
    }

    pub(super) fn assert_guardian_job(&mut self) -> io::Result<()> {
        let guardian = self.guardian();
        if self.job_pid()? != Some(guardian.pid().get()) {
            return Err(io::Error::other("exact job lost original guardian"));
        }
        let live = inspect(guardian.pid().get())?;
        if live.identity != guardian || live.uid != self.spec.uid || live.parent != 1 {
            return Err(io::Error::other("launchd guardian identity changed"));
        }
        Ok(())
    }

    pub(super) fn assert_original_parent(&mut self) -> io::Result<()> {
        let guardian = self
            .guardian
            .ok_or_else(|| io::Error::other("guardian not observed"))?;
        let child = self
            .child
            .ok_or_else(|| io::Error::other("child not observed"))?;
        if self.job_pid()? != Some(guardian.pid().get()) {
            return Err(io::Error::other("exact job lost original guardian"));
        }
        let g = inspect(guardian.pid().get())?;
        let c = inspect(child.pid().get())?;
        if g.identity != guardian
            || g.uid != self.spec.uid
            || g.parent != 1
            || c.identity != child
            || c.uid != self.spec.uid
            || c.parent != guardian.pid().get()
            || c.group != child.pid().get()
        {
            return Err(io::Error::other(
                "original private guardian/child relationship changed",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn cleanup(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        // Recover only fresh output from this new root and this exact job on partial startup.
        if self.child.is_none() {
            self.child = bounded_text(&self.spec.stdout())
                .ok()
                .and_then(|v| child_output(&v));
        }
        // Inspection failure must not prevent an exact-label bootout attempt.
        let mut child_cleanup = Ok(());
        if self.guardian.is_none() {
            match self.job_pid() {
                Ok(Some(pid)) => match inspect(pid) {
                    Ok(live) if live.uid == self.spec.uid => self.guardian = Some(live.identity),
                    Ok(_) => child_cleanup = Err(io::Error::other("wrong guardian UID")),
                    Err(e) => child_cleanup = Err(e),
                },
                Ok(None) => {}
                Err(e) => child_cleanup = Err(e),
            }
        }
        if self.guardian.is_some() && self.child.is_none() {
            child_cleanup = Err(io::Error::other(
                "partial startup has no complete child identity; cleanup unverified",
            ));
        }
        if let Some(child) = self.child {
            match inspect(child.pid().get()) {
                Ok(live) if live.identity == child => {
                    child_cleanup = self.assert_original_parent().and_then(|()| {
                        // SAFETY: freshly checked UID, birth, original parent, and dedicated group;
                        // this is only the harmless child minted by this fixture, never a stored PID.
                        if unsafe {
                            libc::kill(i32::try_from(child.pid().get()).unwrap(), libc::SIGKILL)
                        } != 0
                        {
                            return Err(io::Error::last_os_error());
                        }
                        // The guardian may retain an unreaped zombie until its next observation;
                        // bootout below also lets launchd reap the original parent and orphan.
                        Ok(())
                    });
                }
                Ok(_) => child_cleanup = Err(io::Error::other("child PID reused; refusing signal")),
                Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
                Err(e) => child_cleanup = Err(e),
            }
        }
        let (status, _, stderr) = self.launchctl(LaunchctlAction::Bootout)?;
        // Confirm absence even if bootout reports that a partially bootstrapped job was absent.
        let until = Instant::now() + WAIT;
        while self.job()? != JobObservation::Absent {
            if Instant::now() >= until {
                return Err(io::Error::other(format!(
                    "exact bootout unconfirmed ({status}): {stderr}"
                )));
            }
            poll_pause();
        }
        if let Some(guardian) = self.guardian {
            wait_gone(guardian)?;
        }
        if let Some(child) = self.child {
            wait_gone(child)?;
        }
        child_cleanup?;
        if let Some(summary) = self.cleanup_failures.summary() {
            return Err(io::Error::other(format!(
                "nested fixture cleanup unverified: {summary}"
            )));
        }
        self.armed = false;
        Ok(())
    }

    pub(super) fn finish(mut self) -> io::Result<()> {
        self.cleanup()
    }
}

impl Drop for DisposableLaunchdGuardian {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            // Do not delete evidence (or panic twice) if teardown could not be verified.
            self.spec.root.disable_cleanup(true);
            eprintln!(
                "DISPOSABLE launchd cleanup UNVERIFIED {} at {}: {error}",
                self.spec.target(),
                self.root().display()
            );
        }
    }
}

fn wait_gone(identity: ProcessIdentity) -> io::Result<()> {
    let until = Instant::now() + WAIT;
    let pid = i32::try_from(identity.pid().get())
        .map_err(|_| io::Error::other("PID out of native range"))?;
    loop {
        // SAFETY: signal zero observes existence only; it cannot mutate a reused PID.
        if unsafe { libc::kill(pid, 0) } == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error);
        }
        match inspect(identity.pid().get()) {
            // A zombie can lack libproc data while kill(0) still observes it. Wait for reaping.
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
            Err(e) => return Err(e),
            Ok(live) if live.identity != identity => {
                return Err(io::Error::other("PID reused during teardown"));
            }
            Ok(_) => {}
        }
        if Instant::now() >= until {
            return Err(io::Error::other("process disappearance unconfirmed"));
        }
        poll_pause();
    }
}

const RUNNER_NAMESPACE: &str = "test.disposable.sdc.runner.";

pub(super) struct ByteWitness {
    pub path: PathBuf,
    bytes: Vec<u8>,
    digest: [u8; 32],
}

impl ByteWitness {
    fn capture(path: PathBuf) -> io::Result<Self> {
        let bytes = fs::read(&path)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        Ok(Self {
            path,
            bytes,
            digest,
        })
    }

    pub(super) fn assert_unchanged(&self, uid: u32, mode: u32) -> io::Result<()> {
        let metadata = private_path(&self.path, uid)?;
        if !metadata.is_file() || metadata.mode() & 0o777 != mode {
            return Err(io::Error::other(
                "installed witness lost private regular-file shape",
            ));
        }
        let fresh = Self::capture(self.path.clone())?;
        if fresh.bytes != self.bytes || fresh.digest != self.digest {
            return Err(io::Error::other("installed witness bytes/digest changed"));
        }
        Ok(())
    }
}

fn create_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn stage_binary(source: &Path, target: &Path) -> io::Result<ByteWitness> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(target)?;
    io::copy(&mut input, &mut output)?;
    output.set_permissions(fs::Permissions::from_mode(0o700))?;
    output.sync_all()?;
    let source_bytes = fs::read(source)?;
    let witness = ByteWitness::capture(target.to_owned())?;
    let source_digest: [u8; 32] = Sha256::digest(&source_bytes).into();
    if witness.bytes != source_bytes || witness.digest != source_digest {
        return Err(io::Error::other(
            "staged executable differs from same-build input",
        ));
    }
    Ok(witness)
}

pub(super) struct DisposableLaunchdInstallation {
    guardian: DisposableLaunchdGuardian,
    finished: bool,
    pub guardian_binary: ByteWitness,
    pub runner_binary: ByteWitness,
    pub artifact: ByteWitness,
    pub application_db: ByteWitness,
}

impl DisposableLaunchdInstallation {
    pub(super) fn start() -> io::Result<Self> {
        Self::start_spec(JobSpec::new()?)
    }

    pub(super) fn start_with_timing(
        child_seconds: u64,
        deadline_seconds: u64,
        serve_seconds: u64,
    ) -> io::Result<Self> {
        Self::start_spec(JobSpec::new_with_timing(
            child_seconds,
            deadline_seconds,
            serve_seconds,
        )?)
    }

    fn start_spec(mut spec: JobSpec) -> io::Result<Self> {
        let bin = spec.root.path().join("bin");
        let artifacts = spec.root.path().join("artifacts");
        let data = spec.root.path().join("data");
        for dir in [&bin, &artifacts, &data] {
            create_directory(dir)?;
        }
        let guardian_binary = stage_binary(
            Path::new(env!("CARGO_BIN_EXE_phoenix-deployment-guardian")),
            &bin.join("guardian-test"),
        )?;
        let runner_binary = stage_binary(
            Path::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner")),
            &bin.join("runner-test"),
        )?;
        let artifact_path = artifacts.join("candidate.bin");
        let mut artifact_file = private_file(&artifact_path)?;
        artifact_file.write_all(b"disposable fake candidate artifact; never installed\n")?;
        artifact_file.sync_all()?;
        let artifact = ByteWitness::capture(artifact_path)?;
        let db_path = data.join("application.sqlite");
        let db = rusqlite::Connection::open(&db_path).map_err(io::Error::other)?;
        db.execute_batch("CREATE TABLE installation_marker (value TEXT NOT NULL); INSERT INTO installation_marker VALUES ('fake-application-untouched');")
            .map_err(io::Error::other)?;
        drop(db);
        fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600))?;
        File::open(&db_path)?.sync_all()?;
        let application_db = ByteWitness::capture(db_path)?;
        spec.guardian_program = GuardianProgram::StagedInRoot;
        let guardian = DisposableLaunchdGuardian::start_spec(spec)?;
        let installed = Self {
            guardian,
            finished: false,
            guardian_binary,
            runner_binary,
            artifact,
            application_db,
        };
        installed.assert_witnesses()?;
        Ok(installed)
    }

    pub(super) fn guardian(&mut self) -> &mut DisposableLaunchdGuardian {
        &mut self.guardian
    }

    pub(super) fn assert_witnesses(&self) -> io::Result<()> {
        let uid = self.guardian.spec.uid;
        for dir in ["bin", "artifacts", "data"] {
            let path = self.guardian.root().join(dir);
            if !private_path(&path, uid)?.is_dir() || fs::metadata(path)?.mode() & 0o777 != 0o700 {
                return Err(io::Error::other("installed directory is not private"));
            }
        }
        for witness in [&self.guardian_binary, &self.runner_binary] {
            witness.assert_unchanged(uid, 0o700)?;
        }
        for witness in [&self.artifact, &self.application_db] {
            witness.assert_unchanged(uid, 0o600)?;
        }
        let db = rusqlite::Connection::open_with_flags(
            &self.application_db.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(io::Error::other)?;
        let marker: String = db
            .query_row("SELECT value FROM installation_marker", [], |row| {
                row.get(0)
            })
            .map_err(io::Error::other)?;
        if marker != "fake-application-untouched" {
            return Err(io::Error::other("application witness marker changed"));
        }
        Ok(())
    }

    fn teardown(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        let unwinding = thread::panicking();
        if unwinding {
            self.guardian.spec.root.disable_cleanup(true);
        }
        let mut errors = Vec::new();
        let before = catch_unwind(AssertUnwindSafe(|| self.assert_witnesses()));
        if let Err(error) =
            before.unwrap_or_else(|_| Err(io::Error::other("pre-cleanup witness panicked")))
        {
            errors.push(format!("pre-cleanup witness: {error}"));
        }
        let cleanup = catch_unwind(AssertUnwindSafe(|| self.guardian.cleanup()));
        if let Err(error) =
            cleanup.unwrap_or_else(|_| Err(io::Error::other("guardian cleanup panicked")))
        {
            errors.push(format!("guardian cleanup: {error}"));
        }
        // An attempted but unverified cleanup must not run a second time in guardian Drop.
        self.guardian.armed = false;
        if unwinding {
            self.guardian.spec.root.disable_cleanup(true);
        }
        let after = catch_unwind(AssertUnwindSafe(|| self.assert_witnesses()));
        if let Err(error) =
            after.unwrap_or_else(|_| Err(io::Error::other("post-cleanup witness panicked")))
        {
            errors.push(format!("post-cleanup witness: {error}"));
        }
        self.finished = true;
        if !errors.is_empty() {
            self.guardian.spec.root.disable_cleanup(true);
            for error in &errors {
                self.guardian
                    .cleanup_failures
                    .record(format!("installed witness/guardian teardown: {error}"));
            }
            return Err(io::Error::other(errors.join("; ")));
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> io::Result<()> {
        self.teardown()
    }
}

impl Drop for DisposableLaunchdInstallation {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Err(error) = self.teardown() {
            eprintln!(
                "DISPOSABLE installation cleanup UNVERIFIED at {}: {error}",
                self.guardian.root().display()
            );
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum InstalledRunnerMode {
    InitialPauseAfterApplication,
    RecoverAppliedAck,
    RecoverFinishAndReady,
    RecoverFinishAndChildListPauseAfterApplication,
    RecoverFinishAndChildListPauseAfterObservation,
    RecoverUnappliedChildListPauseAfterVerification,
    RecoverChildListAckPauseAfterConfirmation,
}

struct RunnerJobSpec {
    label: String,
    uid: u32,
    root: PathBuf,
    program: PathBuf,
    socket: PathBuf,
    store: PathBuf,
    deadline: MonotonicTick,
    root_identity: ProcessIdentity,
}

impl RunnerJobSpec {
    fn from_installation(installation: &DisposableLaunchdInstallation) -> Self {
        let mut spec = Self::from_guardian_spec(&installation.guardian.spec);
        spec.root_identity = installation.guardian.child();
        spec
    }

    fn from_guardian_spec(guardian: &JobSpec) -> Self {
        let suffix = guardian.root.path().file_name().unwrap().to_str().unwrap();
        Self {
            label: format!("{RUNNER_NAMESPACE}{}.{suffix}", std::process::id()),
            uid: guardian.uid,
            root: guardian.root.path().to_owned(),
            program: guardian.root.path().join("bin/runner-test"),
            socket: guardian.socket_dir(),
            store: guardian.store(),
            deadline: guardian.deadline,
            root_identity: ProcessIdentity::new(
                ProcessId::new(NonZeroU32::new(7).expect("fixture PID")),
                BirthToken::new(1, 0).expect("fixture birth"),
            ),
        }
    }

    fn target(&self) -> String {
        format!("gui/{}/{}", self.uid, self.label)
    }
    fn plist_path(&self) -> PathBuf {
        self.root.join("runner.plist")
    }
    fn stdout(&self) -> PathBuf {
        self.root.join("runner.stdout")
    }
    fn stderr(&self) -> PathBuf {
        self.root.join("runner.stderr")
    }
    fn release_file(&self) -> PathBuf {
        self.root.join("runner-release.signal")
    }

    fn arguments(&self, mode: InstalledRunnerMode) -> Vec<String> {
        let mut args = vec![
            self.program.to_str().unwrap().into(),
            "--mode".into(),
            match mode {
                InstalledRunnerMode::InitialPauseAfterApplication => "initial-sample",
                InstalledRunnerMode::RecoverAppliedAck => "recover-applied-ack",
                InstalledRunnerMode::RecoverFinishAndReady => "recover-finish-and-ready",
                InstalledRunnerMode::RecoverFinishAndChildListPauseAfterApplication
                | InstalledRunnerMode::RecoverFinishAndChildListPauseAfterObservation => {
                    "recover-finish-and-child-list"
                }
                InstalledRunnerMode::RecoverUnappliedChildListPauseAfterVerification => {
                    "recover-unapplied-child-list"
                }
                InstalledRunnerMode::RecoverChildListAckPauseAfterConfirmation => {
                    "recover-child-list-ack"
                }
            }
            .into(),
            "--socket-dir".into(),
            self.socket.to_str().unwrap().into(),
            "--store".into(),
            self.store.to_str().unwrap().into(),
            "--attempt".into(),
            ATTEMPT.get().to_string(),
        ];
        match mode {
            InstalledRunnerMode::InitialPauseAfterApplication => args.extend([
                "--deadline-nanoseconds".into(),
                self.deadline.get().to_string(),
                "--root-pid".into(),
                self.root_identity.pid().get().to_string(),
                "--root-birth-seconds".into(),
                self.root_identity.birth().seconds().to_string(),
                "--root-birth-nanos".into(),
                self.root_identity.birth().nanos().to_string(),
                "--fixture-pause-after-apply".into(),
                "--fixture-release-file".into(),
                self.release_file().to_str().unwrap().into(),
            ]),
            InstalledRunnerMode::RecoverFinishAndReady => {}
            InstalledRunnerMode::RecoverAppliedAck => args.extend([
                "--fixture-pause-after-recovered-ack".into(),
                "--fixture-release-file".into(),
                self.release_file().to_str().unwrap().into(),
            ]),
            InstalledRunnerMode::RecoverFinishAndChildListPauseAfterApplication => args.extend([
                "--fixture-pause-after-child-list-apply".into(),
                "--fixture-release-file".into(),
                self.release_file().to_str().unwrap().into(),
            ]),
            InstalledRunnerMode::RecoverFinishAndChildListPauseAfterObservation => args.extend([
                "--fixture-pause-after-child-list-observation".into(),
                "--fixture-release-file".into(),
                self.release_file().to_str().unwrap().into(),
            ]),
            InstalledRunnerMode::RecoverUnappliedChildListPauseAfterVerification => args.extend([
                "--fixture-pause-after-recovery-verification".into(),
                "--fixture-release-file".into(),
                self.release_file().to_str().unwrap().into(),
            ]),
            InstalledRunnerMode::RecoverChildListAckPauseAfterConfirmation => args.extend([
                "--fixture-pause-after-child-list-ack".into(),
                "--fixture-release-file".into(),
                self.release_file().to_str().unwrap().into(),
            ]),
        }
        args
    }

    fn assert_private_root(&self) -> io::Result<()> {
        let root = private_path(&self.root, self.uid)?;
        if !root.is_dir() || root.mode() & 0o777 != 0o700 {
            return Err(io::Error::other("runner fixture root is not private"));
        }
        Ok(())
    }

    fn release_temp(&self) -> PathBuf {
        self.root.join("runner-release.signal.tmp")
    }

    fn clear_release_file(&self) -> io::Result<()> {
        self.assert_private_root()?;
        clear_release_temp(self)?;
        remove_private_fixture(&self.release_file(), self.uid)
    }

    fn plist(&self, mode: InstalledRunnerMode) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array>{}</array>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><false/>\n<key>ProcessType</key><string>Background</string>\n<key>Umask</key><integer>63</integer>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            xml(&self.label),
            plist_arguments(&self.arguments(mode)),
            xml(self.stdout().to_str().unwrap()),
            xml(self.stderr().to_str().unwrap())
        )
    }
}

fn assert_private_fixture(path: &Path, uid: u32) -> io::Result<()> {
    let meta = private_path(path, uid)?;
    if !meta.is_file() || meta.mode() & 0o777 != 0o600 {
        return Err(io::Error::other("not a private regular fixture file"));
    }
    Ok(())
}

fn remove_private_fixture(path: &Path, uid: u32) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => {
            assert_private_fixture(path, uid)?;
            fs::remove_file(path)
        }
    }
}

fn clear_release_temp(spec: &RunnerJobSpec) -> io::Result<()> {
    let temp = spec.release_temp();
    match fs::symlink_metadata(&temp) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => {
            assert_private_fixture(&temp, spec.uid)?;
            let bytes = fs::read(&temp)?;
            if bytes != b"" && bytes != b"R" {
                return Err(io::Error::other("unexpected stale runner release bytes"));
            }
            fs::remove_file(temp)
        }
    }
}

fn publish_release(
    spec: &RunnerJobSpec,
    before_rename: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    spec.assert_private_root()?;
    let final_path = spec.release_file();
    let temp = spec.release_temp();
    match fs::symlink_metadata(&final_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => return Err(io::Error::other("runner release already exists")),
    }
    clear_release_temp(spec)?;
    let mut file = private_file(&temp)?;
    file.write_all(b"R")?;
    file.sync_all()?;
    before_rename()?;
    // This root is owned exclusively by the disposable test; never replace a release leaf.
    match fs::symlink_metadata(&final_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => return Err(io::Error::other("runner release appeared before rename")),
    }
    fs::rename(&temp, &final_path)?;
    File::open(&spec.root)?.sync_all()
}

fn archive_initial_file(spec: &RunnerJobSpec, source: &Path, archive: &Path) -> io::Result<()> {
    spec.assert_private_root()?;
    assert_private_fixture(source, spec.uid)?;
    // hard_link creates the destination only if absent, including for a symlink destination.
    fs::hard_link(source, archive)?;
    fs::remove_file(source)?;
    File::open(&spec.root)?.sync_all()
}

pub(super) struct DisposableLaunchdRunnerJob<'a> {
    installation: &'a mut DisposableLaunchdInstallation,
    spec: RunnerJobSpec,
    loaded: bool,
    pid: Option<ProcessIdentity>,
    consumed: usize,
    ctl_sequence: u32,
    cleanup_failures: CleanupFailures,
}

impl<'a> DisposableLaunchdRunnerJob<'a> {
    pub(super) fn start(installation: &'a mut DisposableLaunchdInstallation) -> io::Result<Self> {
        let spec = RunnerJobSpec::from_installation(installation);
        let cleanup_failures = installation.guardian.cleanup_failures();
        let mut job = Self {
            installation,
            spec,
            loaded: false,
            pid: None,
            consumed: 0,
            ctl_sequence: 0,
            cleanup_failures,
        };
        job.bootstrap(InstalledRunnerMode::InitialPauseAfterApplication)?;
        Ok(job)
    }

    pub(super) fn guardian(&mut self) -> &mut DisposableLaunchdGuardian {
        self.installation.guardian()
    }
    pub(super) fn assert_witnesses(&self) -> io::Result<()> {
        self.installation.assert_witnesses()
    }
    pub(super) fn pid(&self) -> ProcessIdentity {
        self.pid.unwrap()
    }

    pub(super) fn label(&self) -> &str {
        &self.spec.label
    }

    fn launchctl(&mut self, args: &[String]) -> io::Result<(ExitStatus, String, String)> {
        self.ctl_sequence += 1;
        let out = self
            .spec
            .root
            .join(format!("runner-ctl-{}.stdout", self.ctl_sequence));
        let err = self
            .spec
            .root
            .join(format!("runner-ctl-{}.stderr", self.ctl_sequence));
        let mut process = LaunchctlProcess {
            child: Command::new("/bin/launchctl")
                .args(args)
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(private_file(&out)?)
                .stderr(private_file(&err)?)
                .spawn()?,
            cleanup_failures: self.cleanup_failures.clone(),
        };
        let until = Instant::now() + WAIT;
        let status = loop {
            if let Some(status) = process.child.try_wait()? {
                break status;
            }
            if Instant::now() >= until {
                return Err(io::Error::other("runner launchctl timed out"));
            }
            poll_pause();
        };
        Ok((status, bounded_text(&out)?, bounded_text(&err)?))
    }

    fn observation(&mut self) -> io::Result<JobObservation> {
        let (status, out, err) = self.launchctl(&["print".into(), self.spec.target()])?;
        JobObservation::from_print(status.success(), &out, &err, &self.spec.label)
    }

    fn bootstrap(&mut self, mode: InstalledRunnerMode) -> io::Result<()> {
        if self.loaded {
            return Err(io::Error::other("runner already loaded"));
        }
        match self.observation()? {
            JobObservation::Absent => {}
            JobObservation::Loaded { .. } => {
                return Err(io::Error::other("runner label already exists"));
            }
        }
        self.spec.clear_release_file()?;
        let plist = self.spec.plist_path();
        if plist.exists() {
            fs::remove_file(&plist)?;
        }
        let mut file = private_file(&plist)?;
        file.write_all(self.spec.plist(mode).as_bytes())?;
        file.sync_all()?;
        for path in [self.spec.stdout(), self.spec.stderr()] {
            if path.exists() {
                fs::remove_file(&path)?;
            }
            drop(private_file(&path)?);
        }
        self.consumed = 0;
        self.pid = None;
        // Mark before launchctl: on partial bootstrap, Drop must attempt exact bootout.
        self.loaded = true;
        let (status, _, stderr) = self.launchctl(&[
            "bootstrap".into(),
            format!("gui/{}", self.spec.uid),
            plist.to_str().unwrap().into(),
        ])?;
        if !status.success() {
            return Err(io::Error::other(format!("runner bootstrap: {stderr}")));
        }
        let until = Instant::now() + WAIT;
        loop {
            if let JobObservation::Loaded { pid: Some(pid) } = self.observation()? {
                let live = inspect(pid)?;
                if live.uid != self.spec.uid || live.parent != 1 {
                    return Err(io::Error::other(
                        "runner is not launchd-owned under current UID",
                    ));
                }
                for path in [
                    self.spec.plist_path(),
                    self.spec.stdout(),
                    self.spec.stderr(),
                ] {
                    if !private_path(&path, self.spec.uid)?.is_file() {
                        return Err(io::Error::other("runner plist/log is not a private file"));
                    }
                }
                self.pid = Some(live.identity);
                return Ok(());
            }
            if Instant::now() >= until {
                return Err(io::Error::other("runner launchd PID not observed"));
            }
            poll_pause();
        }
    }

    pub(super) fn release(&self) -> io::Result<()> {
        publish_release(&self.spec, || Ok(()))
    }

    pub(super) fn marker(&mut self, prefix: &str) -> io::Result<String> {
        self.marker_with_timeout(prefix, WAIT)
    }

    pub(super) fn marker_with_timeout(
        &mut self,
        prefix: &str,
        timeout: Duration,
    ) -> io::Result<String> {
        let until = Instant::now() + timeout;
        loop {
            let output = bounded_text(&self.spec.stdout())?;
            let tail = output
                .get(self.consumed..)
                .ok_or_else(|| io::Error::other("runner output offset invalid"))?;
            if let Some((raw_line, _)) = tail.split_once('\n') {
                self.consumed += raw_line.len() + 1;
                let line = raw_line.trim_end().to_owned();
                if line.starts_with(prefix) {
                    return Ok(line);
                }
                return Err(io::Error::other(format!("expected {prefix}, got {line}")));
            }
            let identity = self.pid();
            let runner_is_live = match inspect(identity.pid().get()) {
                Ok(live) => {
                    live.identity == identity && live.parent == 1 && live.uid == self.spec.uid
                }
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => false,
                Err(error) => return Err(error),
            };
            if !runner_is_live {
                return Err(io::Error::other(format!(
                    "runner exited before {prefix}: {}",
                    bounded_text(&self.spec.stderr())?
                )));
            }
            if Instant::now() >= until {
                return Err(io::Error::other(format!("runner marker timeout: {prefix}")));
            }
            poll_pause();
        }
    }

    pub(super) fn assert_live(&mut self) -> io::Result<()> {
        let identity = self.pid();
        let live = inspect(identity.pid().get())?;
        if live.identity != identity
            || live.parent != 1
            || live.uid != self.spec.uid
            || self.observation()?
                != (JobObservation::Loaded {
                    pid: Some(identity.pid().get()),
                })
        {
            return Err(io::Error::other("exact runner launchd binding changed"));
        }
        Ok(())
    }

    pub(super) fn kill(&mut self) -> io::Result<()> {
        self.assert_live()?;
        let (status, _, err) =
            self.launchctl(&["kill".into(), "SIGKILL".into(), self.spec.target()])?;
        if !status.success() {
            return Err(io::Error::other(format!("runner kill: {err}")));
        }
        wait_gone(self.pid())?;
        let until = Instant::now() + WAIT;
        loop {
            if self.observation()? == (JobObservation::Loaded { pid: None }) {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err(io::Error::other("runner PID still attached after kill"));
            }
            poll_pause();
        }
    }

    fn bootout(&mut self) -> io::Result<()> {
        if !self.loaded {
            return Ok(());
        }
        let (status, _, stderr) = self.launchctl(&["bootout".into(), self.spec.target()])?;
        let until = Instant::now() + WAIT;
        while self.observation()? != JobObservation::Absent {
            if Instant::now() >= until {
                return Err(io::Error::other(format!(
                    "runner bootout unverified ({status}): {stderr}"
                )));
            }
            poll_pause();
        }
        if !status.success() {
            return Err(io::Error::other(format!(
                "exact runner bootout failed: {stderr}"
            )));
        }
        if let Some(pid) = self.pid {
            wait_gone(pid)?;
        }
        self.loaded = false;
        self.pid = None;
        Ok(())
    }

    pub(super) fn assert_archived_generation(&self, generation: u32) -> io::Result<()> {
        for suffix in ["plist", "stdout", "stderr"] {
            let path = self
                .spec
                .root
                .join(format!("runner-generation-{generation}.{suffix}"));
            assert_private_fixture(&path, self.spec.uid)?;
        }
        Ok(())
    }

    pub(super) fn assert_initial_generation_evidence(&self) -> io::Result<()> {
        let archive = self.spec.root.join("runner-generation-1.plist");
        let archived = match fs::symlink_metadata(&archive) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        let paths = if archived {
            [
                archive,
                self.spec.root.join("runner-generation-1.stdout"),
                self.spec.root.join("runner-generation-1.stderr"),
            ]
        } else {
            [
                self.spec.plist_path(),
                self.spec.stdout(),
                self.spec.stderr(),
            ]
        };
        for path in &paths {
            assert_private_fixture(path, self.spec.uid)?;
        }
        let plist = bounded_text(&paths[0])?;
        if !plist.contains("<string>initial-sample</string>") || !plist.contains(&self.spec.label) {
            return Err(io::Error::other("initial generation plist changed"));
        }
        let stdout = bounded_text(&paths[1])?;
        if !stdout.contains("verified binding: InitialActive\n")
            || !stdout.contains("application_committed_pending_ack revision=")
            || !bounded_text(&paths[2])?.is_empty()
        {
            return Err(io::Error::other("initial generation logs changed"));
        }
        Ok(())
    }

    fn archive_generation(&mut self, generation: u32) -> io::Result<()> {
        for (source, archive) in [
            (
                self.spec.plist_path(),
                self.spec
                    .root
                    .join(format!("runner-generation-{generation}.plist")),
            ),
            (
                self.spec.stdout(),
                self.spec
                    .root
                    .join(format!("runner-generation-{generation}.stdout")),
            ),
            (
                self.spec.stderr(),
                self.spec
                    .root
                    .join(format!("runner-generation-{generation}.stderr")),
            ),
        ] {
            archive_initial_file(&self.spec, &source, &archive)?;
        }
        File::open(&self.spec.root)?.sync_all()
    }

    pub(super) fn recover_applied_ack(&mut self) -> io::Result<()> {
        self.bootout().map_err(|error| {
            io::Error::new(error.kind(), format!("runner bootout failed: {error}"))
        })?;
        self.archive_generation(1).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("runner evidence archive failed: {error}"),
            )
        })?;
        self.bootstrap(InstalledRunnerMode::RecoverAppliedAck)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("replacement runner bootstrap failed: {error}"),
                )
            })
    }

    pub(super) fn recover_finish_and_ready(&mut self) -> io::Result<()> {
        self.bootout()?;
        self.archive_generation(2)?;
        self.bootstrap(InstalledRunnerMode::RecoverFinishAndReady)
    }

    pub(super) fn recover_finish_and_child_list(&mut self) -> io::Result<()> {
        self.bootout()?;
        self.archive_generation(2)?;
        self.bootstrap(InstalledRunnerMode::RecoverFinishAndChildListPauseAfterApplication)
    }

    pub(super) fn recover_finish_and_child_list_before_application(&mut self) -> io::Result<()> {
        self.bootout()?;
        self.archive_generation(2)?;
        self.bootstrap(InstalledRunnerMode::RecoverFinishAndChildListPauseAfterObservation)
    }

    pub(super) fn recover_unapplied_child_list(&mut self) -> io::Result<()> {
        self.bootout()?;
        self.archive_generation(3)?;
        self.bootstrap(InstalledRunnerMode::RecoverUnappliedChildListPauseAfterVerification)
    }

    pub(super) fn recover_child_list_ack(&mut self) -> io::Result<()> {
        self.bootout()?;
        self.archive_generation(3)?;
        self.bootstrap(InstalledRunnerMode::RecoverChildListAckPauseAfterConfirmation)
    }

    pub(super) fn success(&mut self) -> io::Result<()> {
        let until = Instant::now() + WAIT;
        loop {
            match self.observation()? {
                JobObservation::Loaded { pid: None } => {
                    wait_gone(self.pid())?;
                    let output = bounded_text(&self.spec.stdout())?;
                    if output.len() != self.consumed
                        || !bounded_text(&self.spec.stderr())?.is_empty()
                    {
                        return Err(io::Error::other(
                            "runner emitted unexpected output or failed",
                        ));
                    }
                    // launchctl print exposes last exit status on a loaded KeepAlive=false job.
                    let (status, print, _) =
                        self.launchctl(&["print".into(), self.spec.target()])?;
                    if !status.success() || !print.contains("last exit code = 0") {
                        return Err(io::Error::other(
                            "runner successful exit not confirmed by launchd",
                        ));
                    }
                    return Ok(());
                }
                JobObservation::Absent => {
                    return Err(io::Error::other("runner job disappeared before bootout"));
                }
                JobObservation::Loaded { .. } => {}
            }
            if Instant::now() >= until {
                return Err(io::Error::other("runner completion timed out"));
            }
            poll_pause();
        }
    }

    pub(super) fn finish(mut self) -> io::Result<()> {
        if let Err(e) = self.bootout() {
            self.cleanup_failures
                .record(format!("runner {} cleanup: {e}", self.spec.target()));
            return Err(e);
        }
        Ok(())
    }
}

impl Drop for DisposableLaunchdRunnerJob<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.bootout() {
            self.cleanup_failures
                .record(format!("runner {} cleanup: {e}", self.spec.target()));
            eprintln!(
                "DISPOSABLE runner cleanup UNVERIFIED at {}: {e}",
                self.spec.root.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_teardown_preserves_invalid_witness_on_finish_and_drop() {
        // No launchctl job is started: this models a failed installation witness.
        for mode in 0..2 {
            let spec = JobSpec::new().unwrap();
            let root = spec.root.path().to_path_buf();
            let path = root.join("incomplete-witness");
            private_file(&path)
                .unwrap()
                .write_all(b"incomplete")
                .unwrap();
            let failures = CleanupFailures::default();
            let installed = DisposableLaunchdInstallation {
                guardian: DisposableLaunchdGuardian {
                    spec,
                    armed: false,
                    guardian: None,
                    child: None,
                    ctl_sequence: 0,
                    cleanup_failures: failures.clone(),
                },
                finished: false,
                guardian_binary: ByteWitness::capture(path.clone()).unwrap(),
                runner_binary: ByteWitness::capture(path.clone()).unwrap(),
                artifact: ByteWitness::capture(path.clone()).unwrap(),
                application_db: ByteWitness::capture(path).unwrap(),
            };
            match mode {
                0 => assert!(installed.finish().is_err()),
                _ => drop(installed),
            }
            assert!(
                root.exists(),
                "invalid witness must retain disposable evidence"
            );
            assert!(failures.summary().unwrap().contains("pre-cleanup witness"));
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn valid_test_installation() -> (DisposableLaunchdInstallation, PathBuf, CleanupFailures) {
        let spec = JobSpec::new().unwrap();
        let root = spec.root.path().to_path_buf();
        for dir in ["bin", "artifacts", "data"] {
            create_directory(&root.join(dir)).unwrap();
        }
        let binary = root.join("bin/fixture");
        fs::write(&binary, b"fixture binary").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let artifact = root.join("artifacts/candidate.bin");
        private_file(&artifact)
            .unwrap()
            .write_all(b"fixture candidate")
            .unwrap();
        let db_path = root.join("data/application.sqlite");
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch("CREATE TABLE installation_marker (value TEXT NOT NULL); INSERT INTO installation_marker VALUES ('fake-application-untouched');").unwrap();
        drop(db);
        fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600)).unwrap();
        let failures = CleanupFailures::default();
        let installed = DisposableLaunchdInstallation {
            guardian: DisposableLaunchdGuardian {
                spec,
                armed: false,
                guardian: None,
                child: None,
                ctl_sequence: 0,
                cleanup_failures: failures.clone(),
            },
            finished: false,
            guardian_binary: ByteWitness::capture(binary.clone()).unwrap(),
            runner_binary: ByteWitness::capture(binary).unwrap(),
            artifact: ByteWitness::capture(artifact).unwrap(),
            application_db: ByteWitness::capture(db_path).unwrap(),
        };
        installed.assert_witnesses().unwrap();
        (installed, root, failures)
    }

    #[test]
    fn valid_installation_finishes_and_removes_root() {
        let (installed, root, failures) = valid_test_installation();
        installed.finish().unwrap();
        assert!(!root.exists());
        assert!(failures.summary().is_none());
    }

    #[test]
    fn unrelated_panic_preserves_valid_installation_without_witness_failure() {
        let (installed, root, failures) = valid_test_installation();
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _installed = installed;
            panic!("unrelated fixture panic");
        }))
        .is_err());
        assert!(root.exists());
        assert!(failures.summary().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_namespace_and_all_mutable_paths_are_disposable() {
        let a = JobSpec::new().unwrap();
        let b = JobSpec::new().unwrap();
        assert_ne!(a.label, b.label);
        assert!(a.label.starts_with(LABEL_NAMESPACE));
        for production in [
            "com.phoenix-ide.server",
            "com.phoenix-ide.deploy",
            "com.phoenix-ide.restart",
        ] {
            assert!(!a.label.starts_with(production));
            assert!(!production.starts_with(&a.label));
        }
        assert_eq!(a.root.path().parent(), Some(Path::new("/tmp")));
        assert_eq!(
            private_path(a.root.path(), a.uid).unwrap().mode() & 0o777,
            0o700
        );
        assert!(a
            .root
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("sdc-disposable-"));
        for path in [
            a.socket_dir(),
            a.store(),
            a.plist_path(),
            a.stdout(),
            a.stderr(),
        ] {
            assert_eq!(path.parent(), Some(a.root.path()));
            assert!(!path.to_str().unwrap().contains(".phoenix-ide"));
            assert!(!path.to_str().unwrap().contains("Library/LaunchAgents"));
        }
        assert_eq!(a.target(), format!("gui/{}/{}", a.uid, a.label));
        assert!(a.socket_dir().join("guardian.sock").as_os_str().len() < 104);
        assert_eq!(
            LaunchctlAction::Bootstrap.arguments(&a),
            vec![
                "bootstrap".to_owned(),
                a.domain(),
                a.plist_path().to_str().unwrap().to_owned()
            ]
        );
        for action in [LaunchctlAction::Print, LaunchctlAction::Bootout] {
            let arguments = action.arguments(&a);
            assert_eq!(arguments.len(), 2);
            assert_eq!(arguments[1], a.target());
        }
        assert_eq!(LaunchctlAction::Bootout.arguments(&a)[0], "bootout");
    }

    #[test]
    fn plist_has_only_direct_fixed_guardian_arguments_and_no_respawn() {
        let spec = JobSpec::new().unwrap();
        let args = spec.arguments();
        assert_eq!(args.len(), 13);
        assert_eq!(args[0], env!("CARGO_BIN_EXE_phoenix-deployment-guardian"));
        assert_eq!(
            Path::new(&args[0]).file_name().unwrap(),
            "phoenix-deployment-guardian"
        );
        assert_eq!(
            &args[1..],
            &[
                "--socket-dir",
                spec.socket_dir().to_str().unwrap(),
                "--child-seconds",
                "110",
                "--serve-seconds",
                "120",
                "--attempt",
                "64",
                "--deadline-nanoseconds",
                &spec.deadline.get().to_string(),
                "--store",
                spec.store().to_str().unwrap()
            ]
        );
        let plist = spec.plist();
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
        assert!(plist.contains("<key>KeepAlive</key><false/>"));
        assert!(plist.contains("<key>ProcessType</key><string>Background</string>"));
        assert!(!plist.contains("<key>Program</key>"));
        for forbidden in [
            "/bin/sh",
            "dev.py",
            "deploy-helper",
            "com.phoenix-ide.",
            "Library/LaunchAgents",
            "~/.phoenix-ide",
        ] {
            assert!(!plist.contains(forbidden), "{forbidden}");
        }
        assert_eq!(xml("a&<b>\"' $;"), "a&amp;&lt;b&gt;&quot;&apos; $;");
        assert_eq!(
            plist_arguments(&["/tmp/build & <x>/guardian \"'".into(), "$; --flag".into()]),
            "<string>/tmp/build &amp; &lt;x&gt;/guardian &quot;&apos;</string><string>$; --flag</string>"
        );
        assert!(plist.contains(&format!(
            "<key>ProgramArguments</key><array>{}</array>",
            plist_arguments(&args)
        )));
    }

    #[test]
    fn runner_generation_clears_only_exact_private_regular_release_file() {
        use std::os::unix::fs::symlink;
        let guardian = JobSpec::new().unwrap();
        let spec = RunnerJobSpec::from_guardian_spec(&guardian);
        let release = spec.release_file();
        assert!(spec.clear_release_file().is_ok());
        let other = spec.root.join("other.signal");
        fs::write(&other, b"untouched").unwrap();
        symlink(&other, &release).unwrap();
        assert!(spec.clear_release_file().is_err());
        assert_eq!(fs::read(&other).unwrap(), b"untouched");
        fs::remove_file(&release).unwrap();
        fs::create_dir(&release).unwrap();
        assert!(spec.clear_release_file().is_err());
        fs::remove_dir(&release).unwrap();
        let mut file = private_file(&release).unwrap();
        file.write_all(b"R").unwrap();
        fs::set_permissions(&release, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(spec.clear_release_file().is_err());
        assert!(release.exists());
        fs::set_permissions(&release, fs::Permissions::from_mode(0o600)).unwrap();
        spec.clear_release_file().unwrap();
        assert!(!release.exists());
        assert_eq!(fs::read(&other).unwrap(), b"untouched");
    }

    #[test]
    fn release_is_invisible_until_complete_private_rename() {
        let guardian = JobSpec::new().unwrap();
        let spec = RunnerJobSpec::from_guardian_spec(&guardian);
        let final_path = spec.release_file();
        publish_release(&spec, || {
            assert!(fs::symlink_metadata(&final_path).is_err());
            assert_private_fixture(&spec.release_temp(), spec.uid)?;
            assert_eq!(fs::read(spec.release_temp())?, b"R");
            Ok(())
        })
        .unwrap();
        assert_private_fixture(&final_path, spec.uid).unwrap();
        assert_eq!(fs::read(&final_path).unwrap(), b"R");
        assert!(fs::symlink_metadata(spec.release_temp()).is_err());
    }

    #[test]
    fn release_refuses_nonfixture_stale_temp() {
        use std::os::unix::fs::symlink;
        for shape in ["symlink", "directory", "nonprivate", "unknown-bytes"] {
            let guardian = JobSpec::new().unwrap();
            let spec = RunnerJobSpec::from_guardian_spec(&guardian);
            let temp = spec.release_temp();
            let other = spec.root.join("untouched");
            fs::write(&other, b"untouched").unwrap();
            match shape {
                "symlink" => symlink(&other, &temp).unwrap(),
                "directory" => fs::create_dir(&temp).unwrap(),
                _ => {
                    private_file(&temp).unwrap().write_all(b"unknown").unwrap();
                    if shape == "nonprivate" {
                        fs::set_permissions(&temp, fs::Permissions::from_mode(0o644)).unwrap();
                    }
                }
            }
            assert!(publish_release(&spec, || Ok(())).is_err(), "{shape}");
            assert!(fs::symlink_metadata(&temp).is_ok(), "{shape}");
            assert!(fs::symlink_metadata(spec.release_file()).is_err());
            assert_eq!(fs::read(other).unwrap(), b"untouched");
        }
    }

    #[test]
    fn archive_refuses_existing_or_symlink_destination_without_removing_source() {
        use std::os::unix::fs::symlink;
        for symlink_destination in [false, true] {
            let guardian = JobSpec::new().unwrap();
            let spec = RunnerJobSpec::from_guardian_spec(&guardian);
            let source = spec.stdout();
            private_file(&source)
                .unwrap()
                .write_all(b"initial output")
                .unwrap();
            let archive = spec.root.join("runner-generation-1.stdout");
            let other = spec.root.join("untouched");
            private_file(&other)
                .unwrap()
                .write_all(b"untouched")
                .unwrap();
            if symlink_destination {
                symlink(&other, &archive).unwrap();
            } else {
                private_file(&archive)
                    .unwrap()
                    .write_all(b"existing")
                    .unwrap();
            }
            assert!(archive_initial_file(&spec, &source, &archive).is_err());
            assert_eq!(fs::read(&source).unwrap(), b"initial output");
            assert_eq!(fs::read(&other).unwrap(), b"untouched");
            assert_eq!(
                fs::read(&archive).unwrap(),
                if symlink_destination {
                    b"untouched".as_slice()
                } else {
                    b"existing".as_slice()
                }
            );
        }
    }

    #[test]
    fn installed_ready_timing_keeps_default_and_private_job_shape() {
        let defaults = JobSpec::new().unwrap();
        assert_eq!(defaults.child_seconds, 110);
        assert_eq!(defaults.serve_seconds, 120);
        assert_eq!(defaults.arguments()[4], "110");
        assert_eq!(defaults.arguments()[6], "120");
        let ready = JobSpec::new_with_timing(30, 60, 70).unwrap();
        assert_eq!(ready.arguments()[4], "30");
        assert_eq!(ready.arguments()[6], "70");
        let now = MonotonicTick::native_now().unwrap();
        assert!(ready.deadline > now);
        assert!(ready.deadline.get().saturating_sub(now.get()) <= 60_000_000_000);
        assert_eq!(ready.root.path().parent(), Some(Path::new("/tmp")));
        assert!(ready.label.starts_with(LABEL_NAMESPACE));
    }

    #[test]
    fn installed_runner_namespace_and_paths_are_private() {
        let mut guardian = JobSpec::new().unwrap();
        guardian.guardian_program = GuardianProgram::StagedInRoot;
        assert_eq!(
            guardian.arguments()[0],
            guardian
                .root
                .path()
                .join("bin/guardian-test")
                .to_str()
                .unwrap()
        );
        assert!(guardian
            .plist()
            .contains(&format!("<string>{}</string>", guardian.arguments()[0])));
        let runner = RunnerJobSpec::from_guardian_spec(&guardian);
        let other = RunnerJobSpec::from_guardian_spec(&JobSpec::new().unwrap());
        assert_ne!(runner.label, other.label);
        assert_eq!(runner.root.parent(), Some(Path::new("/tmp")));
        assert_eq!(runner.root, guardian.root.path());
        assert_eq!(runner.program, guardian.root.path().join("bin/runner-test"));
        assert_eq!(
            runner.plist_path(),
            guardian.root.path().join("runner.plist")
        );
        assert_eq!(
            runner.release_file(),
            guardian.root.path().join("runner-release.signal")
        );
        assert_eq!(runner.stdout(), guardian.root.path().join("runner.stdout"));
        assert_eq!(runner.stderr(), guardian.root.path().join("runner.stderr"));
        assert!(runner.label.starts_with(RUNNER_NAMESPACE));
        assert_ne!(runner.label, guardian.label);
        assert_eq!(
            runner.target(),
            format!("gui/{}/{}", guardian.uid, runner.label)
        );
        assert_eq!(
            plist_arguments(&["a&<\"'>".into()]),
            "<string>a&amp;&lt;&quot;&apos;&gt;</string>"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn installed_runner_modes_and_plist_are_fixed_shapes() {
        let guardian = JobSpec::new().unwrap();
        let runner = RunnerJobSpec::from_guardian_spec(&guardian);
        for mode in [
            InstalledRunnerMode::InitialPauseAfterApplication,
            InstalledRunnerMode::RecoverAppliedAck,
            InstalledRunnerMode::RecoverFinishAndReady,
            InstalledRunnerMode::RecoverFinishAndChildListPauseAfterApplication,
            InstalledRunnerMode::RecoverFinishAndChildListPauseAfterObservation,
            InstalledRunnerMode::RecoverUnappliedChildListPauseAfterVerification,
            InstalledRunnerMode::RecoverChildListAckPauseAfterConfirmation,
        ] {
            let args = runner.arguments(mode);
            assert_eq!(args[0], runner.program.to_str().unwrap());
            assert_eq!(args[1], "--mode");
            assert_eq!(
                &args[3..9],
                [
                    "--socket-dir",
                    runner.socket.to_str().unwrap(),
                    "--store",
                    runner.store.to_str().unwrap(),
                    "--attempt",
                    "64"
                ]
            );
            match mode {
                InstalledRunnerMode::InitialPauseAfterApplication => {
                    assert_eq!(args[2], "initial-sample");
                    assert_eq!(
                        &args[17..],
                        [
                            "--fixture-pause-after-apply",
                            "--fixture-release-file",
                            runner.release_file().to_str().unwrap()
                        ]
                    );
                    assert_eq!(args[12], runner.root_identity.pid().get().to_string());
                    assert_eq!(args[14], runner.root_identity.birth().seconds().to_string());
                    assert_eq!(args[16], runner.root_identity.birth().nanos().to_string());
                }
                InstalledRunnerMode::RecoverAppliedAck => {
                    assert_eq!(args.len(), 12);
                    assert_eq!(args[2], "recover-applied-ack");
                    assert_eq!(
                        &args[9..],
                        [
                            "--fixture-pause-after-recovered-ack",
                            "--fixture-release-file",
                            runner.release_file().to_str().unwrap()
                        ]
                    );
                }
                InstalledRunnerMode::RecoverFinishAndReady => {
                    assert_eq!(args.len(), 9);
                    assert_eq!(args[2], "recover-finish-and-ready");
                }
                InstalledRunnerMode::RecoverFinishAndChildListPauseAfterApplication => {
                    assert_eq!(args.len(), 12);
                    assert_eq!(args[2], "recover-finish-and-child-list");
                    assert_eq!(
                        &args[9..],
                        [
                            "--fixture-pause-after-child-list-apply",
                            "--fixture-release-file",
                            runner.release_file().to_str().unwrap()
                        ]
                    );
                }
                InstalledRunnerMode::RecoverFinishAndChildListPauseAfterObservation => {
                    assert_eq!(args.len(), 12);
                    assert_eq!(args[2], "recover-finish-and-child-list");
                    assert_eq!(
                        &args[9..],
                        [
                            "--fixture-pause-after-child-list-observation",
                            "--fixture-release-file",
                            runner.release_file().to_str().unwrap()
                        ]
                    );
                }
                InstalledRunnerMode::RecoverUnappliedChildListPauseAfterVerification => {
                    assert_eq!(args.len(), 12);
                    assert_eq!(args[2], "recover-unapplied-child-list");
                    assert_eq!(
                        &args[9..],
                        [
                            "--fixture-pause-after-recovery-verification",
                            "--fixture-release-file",
                            runner.release_file().to_str().unwrap()
                        ]
                    );
                }
                InstalledRunnerMode::RecoverChildListAckPauseAfterConfirmation => {
                    assert_eq!(args.len(), 12);
                    assert_eq!(args[2], "recover-child-list-ack");
                    assert_eq!(
                        &args[9..],
                        [
                            "--fixture-pause-after-child-list-ack",
                            "--fixture-release-file",
                            runner.release_file().to_str().unwrap()
                        ]
                    );
                }
            }
            let plist = runner.plist(mode);
            assert!(plist.contains(&format!(
                "<key>ProgramArguments</key><array>{}</array>",
                plist_arguments(&args)
            )));
            assert!(plist.contains("<key>RunAtLoad</key><true/>"));
            assert!(plist.contains("<key>KeepAlive</key><false/>"));
            assert!(!plist.contains("<key>Program</key>"));
            for forbidden in [
                "/bin/sh",
                "com.phoenix-ide.",
                "Library/LaunchAgents",
                ".phoenix-ide",
                "deploy-helper",
            ] {
                assert!(!plist.contains(forbidden), "{forbidden}");
            }
        }
    }

    #[test]
    fn stopped_jobs_and_print_errors_are_not_absence() {
        let label = format!("{LABEL_NAMESPACE}fixture");
        assert_eq!(
            JobObservation::from_print(true, "state = exited\n", "", &label).unwrap(),
            JobObservation::Loaded { pid: None }
        );
        let missing = format!("Could not find service \"{label}\" in domain for user gui: 501");
        assert_eq!(
            JobObservation::from_print(false, "", &missing, &label).unwrap(),
            JobObservation::Absent
        );
        assert!(JobObservation::from_print(false, "", "Could not find domain", &label).is_err());
        assert!(JobObservation::from_print(false, "", "Operation not permitted", &label).is_err());
        assert!(JobObservation::from_print(false, "", &missing, "another-label").is_err());
    }

    #[test]
    fn output_parsers_do_not_accept_partial_or_unrelated_identity() {
        assert!(child_output("disposable child pid=42 birth=7:").is_none());
        assert!(child_output("disposable child pid=42 birth=7:0").is_none());
        assert!(child_output("pid=42 birth=7:0").is_none());
        let child = child_output("disposable child pid=42 birth=7:0\n").unwrap();
        assert_eq!(child.pid().get(), 42);
        assert_eq!(child.birth(), BirthToken::new(7, 0).unwrap());
        assert_eq!(job_pid("\tpid = 123\n"), Some(123));
        assert_eq!(job_pid("last exit code = 123\n"), None);
        assert_eq!(job_pid("pid = 1\n"), None);
        assert_eq!(job_pid("pid = 4294967295\n"), None);
        assert!(child_output("disposable child pid=4294967295 birth=7:0\n").is_none());
    }
}
