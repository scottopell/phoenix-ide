use chrono::{DateTime, Local, NaiveDate};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

const RETAINED_ARCHIVES: usize = 14;
const ROTATION_RETRY_WRITES: u16 = 1024;

enum ArchiveCommand {
    Maintain {
        done: Option<mpsc::Sender<io::Result<()>>>,
    },
    Shutdown,
}

pub(super) struct ArchiveWorkerGuard {
    commands: mpsc::Sender<ArchiveCommand>,
}

impl Drop for ArchiveWorkerGuard {
    fn drop(&mut self) {
        let _ = self.commands.send(ArchiveCommand::Shutdown);
    }
}

impl ArchiveWorkerGuard {
    pub(super) fn start_maintenance(&self) -> io::Result<()> {
        self.commands
            .send(ArchiveCommand::Maintain { done: None })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "archive worker stopped"))
    }
}

pub(super) struct DailyRotatingFile {
    active_path: PathBuf,
    active_date: NaiveDate,
    active_permissions: fs::Permissions,
    file: Option<File>,
    archive_commands: mpsc::Sender<ArchiveCommand>,
    _rotation_lock: File,
    rotation_disabled: bool,
    rotation_retry_writes: u16,
}

impl DailyRotatingFile {
    pub(super) fn open(path: &Path) -> io::Result<(Self, ArchiveWorkerGuard)> {
        let active_date = fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .map(DateTime::<Local>::from)
            .map(|modified| modified.date_naive())
            .unwrap_or_else(|_| Local::now().date_naive());
        Self::open_with_date(path, active_date)
    }

    fn open_with_date(
        path: &Path,
        active_date: NaiveDate,
    ) -> io::Result<(Self, ArchiveWorkerGuard)> {
        reject_non_regular_active_path(path)?;
        validate_rotation_directory(path)?;
        let rotation_lock = open_rotation_lock(path)?;
        let file = open_append(path)?;
        if !file.metadata()?.is_file() {
            return Err(non_regular_active_path(path));
        }
        let active_permissions = file.metadata()?.permissions();
        let active_path = path.to_path_buf();
        let worker_path = active_path.clone();
        let worker_lock = rotation_lock.try_clone()?;
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("phoenix-log-archive".to_string())
            .spawn(move || {
                let _lock = worker_lock;
                archive_worker(&worker_path, &receiver);
            })?;
        let guard = ArchiveWorkerGuard {
            commands: commands.clone(),
        };
        let writer = Self {
            active_path,
            active_date,
            active_permissions,
            file: Some(file),
            archive_commands: commands,
            _rotation_lock: rotation_lock,
            rotation_disabled: false,
            rotation_retry_writes: 0,
        };
        Ok((writer, guard))
    }

    fn rotate(&mut self, new_date: NaiveDate) -> io::Result<()> {
        self.rotate_with_archive_move(new_date, |source, target| fs::rename(source, target))
    }

    fn rotate_with_archive_move(
        &mut self,
        new_date: NaiveDate,
        move_active: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("active structured log is not open"))?
            .flush()?;
        let archive_path = next_archive_path(&self.active_path, self.active_date)?;
        let current = self
            .file
            .take()
            .expect("active structured log checked above");
        if let Err(error) = move_active(&self.active_path, &archive_path) {
            self.file = Some(current);
            return Err(error);
        }

        match open_new_with_permissions(&self.active_path, &self.active_permissions) {
            Ok(file) => {
                self.file = Some(file);
                self.active_date = new_date;
                self.request_maintenance(None);
                Ok(())
            }
            Err(create_error) => {
                let restore_error = fs::rename(&archive_path, &self.active_path).err();
                self.rotation_disabled = restore_error.is_some();
                self.file = Some(current);
                if let Some(restore_error) = restore_error {
                    Err(io::Error::new(
                        create_error.kind(),
                        format!(
                            "replacement log creation failed: {create_error}; archive restore failed: {restore_error}; rotation disabled"
                        ),
                    ))
                } else {
                    Err(create_error)
                }
            }
        }
    }

    fn request_maintenance(&self, done: Option<mpsc::Sender<io::Result<()>>>) {
        if self
            .archive_commands
            .send(ArchiveCommand::Maintain { done })
            .is_err()
        {
            tracing::error!("Phoenix log archive worker stopped unexpectedly");
        }
    }

    fn write_on_date(&mut self, buffer: &[u8], date: NaiveDate) -> io::Result<usize> {
        if !self.rotation_disabled && date != self.active_date {
            if self.rotation_retry_writes == 0 {
                if let Err(error) = self.rotate(date) {
                    self.rotation_retry_writes = ROTATION_RETRY_WRITES;
                    super::record_fatal_diagnostic(&format!(
                        "log rotation failed for {}: {error}",
                        self.active_path.display()
                    ));
                }
            } else {
                self.rotation_retry_writes -= 1;
            }
        }
        if self.file.is_none() {
            self.file = Some(open_append_with_permissions(
                &self.active_path,
                &self.active_permissions,
            )?);
        }
        self.file
            .as_mut()
            .expect("file restored above")
            .write(buffer)
    }

    #[cfg(test)]
    fn wait_for_maintenance(&self) -> io::Result<()> {
        let (done, completed) = mpsc::channel();
        self.request_maintenance(Some(done));
        completed
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "archive worker stopped"))?
    }
}

impl Write for DailyRotatingFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.write_on_date(buffer, Local::now().date_naive())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), Write::flush)
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    super::open_log_append(path, None)
}

fn validate_rotation_directory(path: &Path) -> io::Result<()> {
    let parent = parent_directory(path);
    let metadata = fs::metadata(parent)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "log rotation parent is not a directory: {}",
                parent.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: `geteuid` has no preconditions.
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o002 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "log rotation directory must be process-owned and not world-writable: {}",
                    parent.display()
                ),
            ));
        }
    }
    Ok(())
}

fn reject_non_regular_active_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(non_regular_active_path(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn non_regular_active_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "rotating log path must be a regular file: {}",
            path.display()
        ),
    )
}

fn open_rotation_lock(active_path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    let (lock, lock_target) = {
        let directory = parent_directory(active_path);
        (File::open(directory)?, directory.to_path_buf())
    };
    #[cfg(not(unix))]
    let (lock, lock_target) = {
        let path = reserved_sibling_lock_path(active_path);
        (super::open_log_append(&path, Some(0o600))?, path)
    };
    #[cfg(not(unix))]
    if !lock.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "log rotation lock must be a regular file: {}",
                lock_target.display()
            ),
        ));
    }
    FileExt::try_lock_exclusive(&lock).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "another process owns log rotation in {}: {error}",
                lock_target.display()
            ),
        )
    })?;
    Ok(lock)
}

fn open_append_with_permissions(path: &Path, permissions: &fs::Permissions) -> io::Result<File> {
    #[cfg(unix)]
    let create_mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(permissions.mode())
    };
    #[cfg(not(unix))]
    let create_mode = None;
    let file = super::open_log_append(path, create_mode)?;
    file.set_permissions(permissions.clone())?;
    Ok(file)
}

fn open_new_with_permissions(path: &Path, permissions: &fs::Permissions) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options
            .mode(permissions.mode())
            .custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    file.set_permissions(permissions.clone())?;
    Ok(file)
}

fn archive_worker(active_path: &Path, commands: &mpsc::Receiver<ArchiveCommand>) {
    while let Ok(command) = commands.recv() {
        match command {
            ArchiveCommand::Maintain { done } => {
                let result = maintain_archives(active_path);
                if let Some(done) = done {
                    let _ = done.send(result);
                } else if let Err(error) = result {
                    tracing::error!(%error, "Phoenix log archive maintenance failed");
                }
            }
            ArchiveCommand::Shutdown => break,
        }
    }
}

fn maintain_archives(active_path: &Path) -> io::Result<()> {
    maintain_archives_with(active_path, compress_archive)
}

fn maintain_archives_with(
    active_path: &Path,
    mut compress: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let mut first_error = None;
    if let Err(error) = remove_stale_archive_temporaries(active_path) {
        first_error.get_or_insert(error);
    }
    for archive in find_archives(active_path)? {
        if !archive_is_compressed(&archive) {
            if let Err(error) = compress(active_path, &archive) {
                first_error.get_or_insert(error);
            }
        }
    }

    let mut archives = find_archives(active_path)?;
    archives.sort_by_key(|path| archive_generation(active_path, path));
    let mut remove_count = archives.len().saturating_sub(RETAINED_ARCHIVES);
    for archive in archives {
        if remove_count == 0 {
            break;
        }
        match fs::remove_file(archive) {
            Ok(()) => remove_count -= 1,
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn remove_stale_archive_temporaries(active_path: &Path) -> io::Result<()> {
    let parent = parent_directory(active_path);
    let prefix = archive_temp_prefix(active_path);
    let mut first_error = None;
    for entry in fs::read_dir(parent)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        if !entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(prefix.as_bytes())
        {
            continue;
        }
        match entry.file_type() {
            Ok(file_type) if file_type.is_file() || file_type.is_symlink() => {
                if let Err(error) = fs::remove_file(entry.path()) {
                    first_error.get_or_insert(error);
                }
            }
            Ok(_) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn find_archives(active_path: &Path) -> io::Result<Vec<PathBuf>> {
    let parent = parent_directory(active_path);
    let mut archives = Vec::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if archive_suffix(active_path, &path).is_some_and(valid_archive_suffix) {
            archives.push(path);
        }
    }
    Ok(archives)
}

fn archive_suffix<'a>(active_path: &Path, archive_path: &'a Path) -> Option<&'a [u8]> {
    let active_name = active_path.file_name()?.as_encoded_bytes();
    let archive_name = archive_path.file_name()?.as_encoded_bytes();
    archive_name.strip_prefix(active_name)?.strip_prefix(b".")
}

fn valid_archive_suffix(suffix: &[u8]) -> bool {
    let raw = suffix.strip_suffix(b".gz").unwrap_or(suffix);
    raw.len() == 31
        && raw[..20].iter().all(u8::is_ascii_digit)
        && raw[20] == b'-'
        && raw[25] == b'-'
        && raw[28] == b'-'
        && raw[21..]
            .iter()
            .copied()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn archive_generation(active_path: &Path, archive_path: &Path) -> Option<u64> {
    let suffix = archive_suffix(active_path, archive_path)?;
    let raw = suffix.strip_suffix(b".gz").unwrap_or(suffix);
    if !valid_archive_suffix(suffix) {
        return None;
    }
    std::str::from_utf8(raw.get(..20)?).ok()?.parse().ok()
}

fn next_archive_path(active_path: &Path, date: NaiveDate) -> io::Result<PathBuf> {
    let generation = find_archives(active_path)?
        .iter()
        .filter_map(|path| archive_generation(active_path, path))
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| io::Error::other("log archive generation exhausted"))?;
    let candidate = path_with_suffix(
        active_path,
        &format!(".{generation:020}-{}", date.format("%Y-%m-%d")),
    );
    if candidate.exists() || path_with_suffix(&candidate, ".gz").exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "next log archive generation already exists",
        ));
    }
    Ok(candidate)
}

fn archive_is_compressed(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().ends_with(b".gz"))
}

fn compress_archive(active_path: &Path, source: &Path) -> io::Result<()> {
    let target = path_with_suffix(source, ".gz");
    if target.exists() {
        if gzip_matches_source(source, &target)? {
            return fs::remove_file(source);
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("gzip target differs from source: {}", target.display()),
        ));
    }
    let parent = parent_directory(&target);
    let mut input = open_read_nofollow(source)?;
    let source_permissions = input.metadata()?.permissions();
    let mut temporary = tempfile::Builder::new()
        .prefix(&archive_temp_prefix(active_path))
        .tempfile_in(parent)?;
    temporary.as_file().set_permissions(source_permissions)?;
    {
        let mut encoder = GzEncoder::new(temporary.as_file_mut(), Compression::default());
        io::copy(&mut input, &mut encoder)?;
        let output = encoder.finish()?;
        output.sync_all()?;
    }
    temporary
        .persist_noclobber(&target)
        .map_err(|error| error.error)?;
    fs::remove_file(source)
}

fn gzip_matches_source(source: &Path, target: &Path) -> io::Result<bool> {
    Ok(reader_digest(open_read_nofollow(source)?)?
        == reader_digest(GzDecoder::new(open_read_nofollow(target)?))?)
}

fn reader_digest(mut reader: impl Read) -> io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(digest.finalize().into());
        }
        digest.update(&buffer[..read]);
    }
}

fn open_read_nofollow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("log archive must be a regular file: {}", path.display()),
        ));
    }
    Ok(file)
}

fn archive_temp_prefix(active_path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let name = active_path
        .file_name()
        .unwrap_or_default()
        .as_encoded_bytes();
    let digest = Sha256::digest(name);
    let mut identity = String::with_capacity(32);
    for byte in &digest[..16] {
        write!(&mut identity, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!(".phoenix-log-archive-{identity}-")
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub(super) fn reserved_sibling_lock_path(active_path: &Path) -> PathBuf {
    path_with_suffix(active_path, ".rotation.lock")
}

pub(super) fn path_is_reserved(active_path: &Path, candidate: &Path) -> io::Result<bool> {
    if super::paths_alias(&reserved_sibling_lock_path(active_path), candidate)? {
        return Ok(true);
    }
    if !super::paths_alias(parent_directory(active_path), parent_directory(candidate))? {
        return Ok(false);
    }
    if archive_suffix(active_path, candidate).is_some_and(valid_archive_suffix) {
        return Ok(true);
    }
    Ok(candidate.file_name().is_some_and(|name| {
        name.as_encoded_bytes()
            .starts_with(archive_temp_prefix(active_path).as_bytes())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn archive_path(
        active: &Path,
        generation: u64,
        archive_date: NaiveDate,
        compressed: bool,
    ) -> PathBuf {
        let suffix = format!(
            ".{generation:020}-{}{}",
            archive_date.format("%Y-%m-%d"),
            if compressed { ".gz" } else { "" }
        );
        path_with_suffix(active, &suffix)
    }

    #[test]
    fn day_change_swaps_active_file_before_compressing_archive() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let (mut writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();

        writer
            .write_on_date(b"old day\n", date(2026, 8, 9))
            .unwrap();
        writer
            .write_on_date(b"new day\n", date(2026, 8, 10))
            .unwrap();
        writer.flush().unwrap();
        writer.wait_for_maintenance().unwrap();

        assert_eq!(fs::read_to_string(&active).unwrap(), "new day\n");
        let archive = archive_path(&active, 1, date(2026, 8, 9), true);
        assert!(!archive_path(&active, 1, date(2026, 8, 9), false).exists());
        let mut archived = String::new();
        GzDecoder::new(File::open(archive).unwrap())
            .read_to_string(&mut archived)
            .unwrap();
        assert_eq!(archived, "old day\n");
    }

    #[test]
    fn explicit_initial_maintenance_compresses_existing_archives() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let archive = archive_path(&active, 1, date(2026, 8, 8), false);
        fs::write(&archive, "old\n").unwrap();
        let (writer, guard) = DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();

        guard.start_maintenance().unwrap();
        writer.wait_for_maintenance().unwrap();

        assert!(!archive.exists());
        assert!(archive_path(&active, 1, date(2026, 8, 8), true).exists());
    }

    #[test]
    fn second_rotator_for_the_same_active_log_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let (_writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();

        let Err(error) = DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)) else {
            panic!("second rotator unexpectedly acquired the same log");
        };

        assert!(error
            .to_string()
            .contains("another process owns log rotation"));
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_active_log_is_rejected() {
        let Err(error) = DailyRotatingFile::open(Path::new("/dev/null")) else {
            panic!("special file unexpectedly accepted for rotation");
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("regular file"));
    }

    #[cfg(unix)]
    #[test]
    fn shared_rotation_directory_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o777)).unwrap();
        let active = temp.path().join("prod.log");

        let Err(error) = DailyRotatingFile::open(&active) else {
            panic!("shared directory unexpectedly accepted for rotation");
        };

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("not world-writable"));
    }

    #[cfg(unix)]
    #[test]
    fn group_writable_owned_rotation_directory_is_allowed() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o775)).unwrap();
        let active = temp.path().join("phoenix.log");

        let (_writer, _guard) = DailyRotatingFile::open(&active).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_sibling_file_cannot_enable_a_second_rotator() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o775)).unwrap();
        let active = temp.path().join("phoenix.log");
        let (_writer, _guard) = DailyRotatingFile::open(&active).unwrap();
        let replaceable_sibling = path_with_suffix(&active, ".rotation.lock");
        if let Err(error) = fs::remove_file(&replaceable_sibling) {
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
        }
        fs::write(replaceable_sibling, "replacement").unwrap();

        let Err(error) = DailyRotatingFile::open(&active) else {
            panic!("second rotator unexpectedly bypassed directory ownership");
        };

        assert!(error
            .to_string()
            .contains("another process owns log rotation"));
    }

    #[cfg(unix)]
    #[test]
    fn rotation_ownership_is_scoped_to_the_log_directory() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.log");
        let second = temp.path().join("second.log");
        let (_writer, _guard) = DailyRotatingFile::open(&first).unwrap();

        let Err(error) = DailyRotatingFile::open(&second) else {
            panic!("second rotator unexpectedly acquired the same log directory");
        };

        assert!(error
            .to_string()
            .contains("another process owns log rotation"));
    }

    #[test]
    fn initial_maintenance_removes_abandoned_archive_temporaries() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let abandoned = temp
            .path()
            .join(format!("{}abandoned", archive_temp_prefix(&active)));
        let other_active = temp.path().join("other.log");
        let other = temp
            .path()
            .join(format!("{}in-progress", archive_temp_prefix(&other_active)));
        fs::write(&abandoned, "partial archive").unwrap();
        fs::write(&other, "live archive").unwrap();

        maintain_archives(&active).unwrap();

        assert!(!abandoned.exists());
        assert!(other.exists());
    }

    #[test]
    fn archive_maintenance_keeps_newest_fourteen_generations() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let (mut writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 1)).unwrap();

        for day in 1..=16 {
            writer
                .write_on_date(format!("day {day}\n").as_bytes(), date(2026, 8, day))
                .unwrap();
        }
        writer.flush().unwrap();
        writer.wait_for_maintenance().unwrap();

        let archives = find_archives(&active).unwrap();
        assert_eq!(archives.len(), RETAINED_ARCHIVES);
        assert!(!archive_path(&active, 1, date(2026, 8, 1), true).exists());
        assert!(archive_path(&active, 2, date(2026, 8, 2), true).exists());
        assert!(archive_path(&active, 15, date(2026, 8, 15), true).exists());
    }

    #[test]
    fn pruning_enforces_retention_when_every_compression_fails() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        for day in 1..=16 {
            fs::write(
                archive_path(&active, day.into(), date(2026, 8, day), false),
                "archive\n",
            )
            .unwrap();
        }

        assert!(maintain_archives_with(&active, |_, _| Err(io::Error::other(
            "injected compression failure"
        )))
        .is_err());

        assert_eq!(find_archives(&active).unwrap().len(), RETAINED_ARCHIVES);
        assert!(!archive_path(&active, 1, date(2026, 8, 1), false).exists());
        assert!(!archive_path(&active, 2, date(2026, 8, 2), false).exists());
        assert!(archive_path(&active, 3, date(2026, 8, 3), false).exists());
    }

    #[cfg(unix)]
    #[test]
    fn compressed_archive_preserves_source_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let source = archive_path(&active, 1, date(2026, 8, 9), false);
        fs::write(&source, "secret\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();

        compress_archive(&active, &source).unwrap();

        let mode = fs::metadata(archive_path(&active, 1, date(2026, 8, 9), true))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn predictable_temporary_symlink_cannot_redirect_compression() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let source = archive_path(&active, 1, date(2026, 8, 9), false);
        let victim = temp.path().join("victim");
        fs::write(&source, "archive\n").unwrap();
        fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(&victim, path_with_suffix(&source, ".gz.tmp")).unwrap();

        compress_archive(&active, &source).unwrap();

        assert_eq!(fs::read_to_string(victim).unwrap(), "keep me");
        assert!(path_with_suffix(&source, ".gz").is_file());
    }

    #[test]
    fn published_gzip_recovers_source_left_by_process_exit() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let source = archive_path(&active, 1, date(2026, 8, 9), false);
        fs::write(&source, "archive\n").unwrap();
        compress_archive(&active, &source).unwrap();
        fs::write(&source, "archive\n").unwrap();

        compress_archive(&active, &source).unwrap();

        assert!(!source.exists());
        assert!(path_with_suffix(&source, ".gz").exists());
    }

    #[test]
    fn pruning_uses_generation_when_calendar_dates_move_backward() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        for generation in 1..=15 {
            let day = if generation % 2 == 0 { 1 } else { 20 };
            fs::write(
                archive_path(&active, generation, date(2026, 8, day), true),
                "archive\n",
            )
            .unwrap();
        }

        maintain_archives(&active).unwrap();

        assert!(!archive_path(&active, 1, date(2026, 8, 20), true).exists());
        assert!(archive_path(&active, 2, date(2026, 8, 1), true).exists());
    }

    #[cfg(unix)]
    #[test]
    fn active_log_preserves_permissions_across_rotation() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        fs::write(&active, "old\n").unwrap();
        fs::set_permissions(&active, fs::Permissions::from_mode(0o600)).unwrap();
        let (mut writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();

        writer.write_on_date(b"new\n", date(2026, 8, 10)).unwrap();
        writer.wait_for_maintenance().unwrap();

        let mode = fs::metadata(active).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_log_is_created_exclusively_with_preserved_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let permissions = fs::Permissions::from_mode(0o600);

        let file = open_new_with_permissions(&active, &permissions).unwrap();

        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            open_new_with_permissions(&active, &permissions)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn rotation_failure_keeps_appending_to_active_file() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let (mut writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();
        writer.write_all(b"old\n").unwrap();

        let error = writer
            .rotate_with_archive_move(date(2026, 8, 10), |_, _| {
                Err(io::Error::other("injected archive move failure"))
            })
            .unwrap_err();
        writer.file.as_mut().unwrap().write_all(b"new\n").unwrap();

        assert_eq!(error.to_string(), "injected archive move failure");
        writer.flush().unwrap();
        assert_eq!(fs::read_to_string(active).unwrap(), "old\nnew\n");
    }

    #[test]
    fn rotation_retry_backoff_is_bounded_by_written_records() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let (mut writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();
        writer.rotation_retry_writes = 2;

        writer.write_on_date(b"one\n", date(2026, 8, 10)).unwrap();
        writer.write_on_date(b"two\n", date(2026, 8, 10)).unwrap();
        writer.flush().unwrap();

        assert_eq!(writer.rotation_retry_writes, 0);
        assert_eq!(fs::read_to_string(active).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn fatal_paths_are_rejected_from_every_rotation_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let archive = archive_path(&active, 1, date(2026, 8, 9), false);
        let temporary = temp
            .path()
            .join(format!("{}partial", archive_temp_prefix(&active)));

        assert!(path_is_reserved(&active, &reserved_sibling_lock_path(&active)).unwrap());
        assert!(path_is_reserved(&active, &archive).unwrap());
        assert!(path_is_reserved(&active, &temporary).unwrap());
        assert!(!path_is_reserved(&active, &temp.path().join("fatal.log")).unwrap());
    }

    #[test]
    fn replacement_collision_restores_the_active_log_and_keeps_writing() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let (mut writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();
        writer.write_all(b"old\n").unwrap();

        let error = writer
            .rotate_with_archive_move(date(2026, 8, 10), |source, target| {
                fs::rename(source, target)?;
                fs::write(source, "collision\n")
            })
            .unwrap_err();
        writer.write_all(b"new\n").unwrap();
        writer.flush().unwrap();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(active).unwrap(), "old\nnew\n");
    }

    #[cfg(unix)]
    #[test]
    fn fifo_with_archive_name_is_ignored_and_rejected_if_opened() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let fifo = archive_path(&active, 1, date(2026, 8, 9), false);
        let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);

        assert!(find_archives(&active).unwrap().is_empty());
        assert_eq!(
            open_read_nofollow(&fifo).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[cfg(unix)]
    #[test]
    fn archive_matching_preserves_non_utf8_filename_identity() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let active = PathBuf::from(OsString::from_vec(b"prod-\xff.log".to_vec()));
        let owned = archive_path(&active, 1, date(2026, 8, 9), true);
        let unrelated = PathBuf::from(OsString::from_vec(
            b"prod-\xfe.log.00000000000000000002-2026-08-09.gz".to_vec(),
        ));

        assert!(archive_suffix(&active, &owned).is_some_and(valid_archive_suffix));
        assert!(archive_suffix(&active, &unrelated).is_none());
    }

    #[test]
    fn archive_names_do_not_match_unrelated_files() {
        assert!(valid_archive_suffix(b"00000000000000000001-2026-08-09"));
        assert!(valid_archive_suffix(b"00000000000000000002-2026-08-09.gz"));
        assert!(!valid_archive_suffix(b"0.bz2"));
        assert!(!valid_archive_suffix(
            b"00000000000000000001-2026-08-09.tmp"
        ));
        assert!(!valid_archive_suffix(b"2026-08-09"));
        assert!(!valid_archive_suffix(b"not-a-date.gz"));
    }

    #[test]
    fn bare_relative_log_uses_the_current_directory() {
        assert_eq!(parent_directory(Path::new("prod.log")), Path::new("."));
    }
}
