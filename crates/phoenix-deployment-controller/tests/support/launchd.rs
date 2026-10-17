#![cfg(target_os = "macos")]

use phoenix_deployment_controller::{
    AttemptId, BirthToken, ControllerSqliteStore, MonotonicTick, ProcessId, ProcessIdentity,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem::{size_of, MaybeUninit},
    num::NonZeroU32,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
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

struct JobSpec {
    root: tempfile::TempDir,
    label: String,
    uid: u32,
    deadline: MonotonicTick,
}

impl JobSpec {
    fn new() -> io::Result<Self> {
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
            .checked_add(Duration::from_secs(90))
            .ok_or_else(|| io::Error::other("deadline overflow"))?;
        Ok(Self {
            root,
            label,
            uid,
            deadline,
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
            env!("CARGO_BIN_EXE_phoenix-deployment-guardian").to_owned(),
            "--socket-dir".into(),
            self.socket_dir().to_str().unwrap().into(),
            "--child-seconds".into(),
            "110".into(),
            "--serve-seconds".into(),
            "120".into(),
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
        let spec = JobSpec::new()?;
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
                "disposable bootstrap failed in {domain}; requires this UID's logged-in GUI domain (no system/user fallback, sudo, or silent skip): {stderr}")));
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
                return Err(io::Error::other("PID reused during teardown"))
            }
            Ok(_) => {}
        }
        if Instant::now() >= until {
            return Err(io::Error::other("process disappearance unconfirmed"));
        }
        poll_pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(plist_arguments(&["/tmp/build & <x>/guardian \"'".into(), "$; --flag".into()]),
            "<string>/tmp/build &amp; &lt;x&gt;/guardian &quot;&apos;</string><string>$; --flag</string>");
        assert!(plist.contains(&format!(
            "<key>ProgramArguments</key><array>{}</array>",
            plist_arguments(&args)
        )));
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
