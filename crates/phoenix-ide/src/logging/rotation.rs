use chrono::{DateTime, Local, NaiveDate};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

const RETAINED_ARCHIVES: usize = 14;
const ARCHIVE_TEMP_PREFIX: &str = ".phoenix-log-archive-";

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
        let file = open_append(path)?;
        let active_permissions = file.metadata()?.permissions();
        let active_path = path.to_path_buf();
        let worker_path = active_path.clone();
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("phoenix-log-archive".to_string())
            .spawn(move || archive_worker(&worker_path, &receiver))?;
        let guard = ArchiveWorkerGuard {
            commands: commands.clone(),
        };
        let writer = Self {
            active_path,
            active_date,
            active_permissions,
            file: Some(file),
            archive_commands: commands,
        };
        Ok((writer, guard))
    }

    fn rotate(&mut self, new_date: NaiveDate) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }
        drop(self.file.take());

        let archive_path = next_archive_path(&self.active_path, self.active_date);
        if let Err(error) = fs::rename(&self.active_path, &archive_path) {
            self.file =
                open_append_with_permissions(&self.active_path, &self.active_permissions).ok();
            return Err(error);
        }

        match open_append_with_permissions(&self.active_path, &self.active_permissions) {
            Ok(file) => {
                self.file = Some(file);
                self.active_date = new_date;
                self.request_maintenance(None);
                Ok(())
            }
            Err(error) => {
                let _ = fs::rename(&archive_path, &self.active_path);
                self.file =
                    open_append_with_permissions(&self.active_path, &self.active_permissions).ok();
                Err(error)
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
        if date != self.active_date {
            if let Err(error) = self.rotate(date) {
                super::record_fatal_diagnostic(&format!(
                    "log rotation failed for {}: {error}",
                    self.active_path.display()
                ));
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
    let mut first_error = None;
    if let Err(error) = remove_stale_archive_temporaries(active_path) {
        first_error.get_or_insert(error);
    }
    let mut failed_compression = std::collections::HashSet::new();
    for archive in find_archives(active_path)? {
        if !archive_is_compressed(&archive) {
            if let Err(error) = compress_archive(&archive) {
                failed_compression.insert(archive);
                first_error.get_or_insert(error);
            }
        }
    }

    let mut archives = find_archives(active_path)?;
    archives.sort_by_key(|path| path.file_name().map(ToOwned::to_owned));
    let mut remove_count = archives.len().saturating_sub(RETAINED_ARCHIVES);
    for archive in archives {
        if remove_count == 0 {
            break;
        }
        if failed_compression.contains(&archive) {
            continue;
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
    let parent = active_path.parent().unwrap_or_else(|| Path::new("."));
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
            .to_string_lossy()
            .starts_with(ARCHIVE_TEMP_PREFIX)
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
    let Some(active_name) = active_path.file_name() else {
        return Ok(Vec::new());
    };
    let prefix = format!("{}.", active_name.to_string_lossy());
    let parent = active_path.parent().unwrap_or_else(|| Path::new("."));
    let mut archives = Vec::new();
    for entry in fs::read_dir(parent)? {
        let path = entry?.path();
        let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
            continue;
        };
        if name.strip_prefix(&prefix).is_some_and(valid_archive_suffix) {
            archives.push(path);
        }
    }
    Ok(archives)
}

fn valid_archive_suffix(suffix: &str) -> bool {
    let raw = suffix.strip_suffix(".gz").unwrap_or(suffix);
    let bytes = raw.as_bytes();
    let date_valid = bytes.len() >= 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..10]
            .iter()
            .copied()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
    if !date_valid {
        return false;
    }
    let collision = &bytes[10..];
    let collision_valid = collision.is_empty()
        || (collision.first() == Some(&b'-')
            && collision.len() > 1
            && collision[1..].iter().all(u8::is_ascii_digit));
    date_valid && collision_valid
}

fn next_archive_path(active_path: &Path, date: NaiveDate) -> PathBuf {
    let base = path_with_suffix(active_path, &format!(".{}", date.format("%Y-%m-%d")));
    for collision in 0_u32.. {
        let suffix = if collision == 0 {
            String::new()
        } else {
            format!("-{collision}")
        };
        let candidate = path_with_suffix(&base, &suffix);
        let compressed = path_with_suffix(&candidate, ".gz");
        if !candidate.exists() && !compressed.exists() {
            return candidate;
        }
    }
    unreachable!("u32 archive collision space exhausted")
}

fn archive_is_compressed(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".gz"))
}

fn compress_archive(source: &Path) -> io::Result<()> {
    let target = path_with_suffix(source, ".gz");
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let mut input_options = OpenOptions::new();
    input_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        input_options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut input = input_options.open(source)?;
    let source_permissions = input.metadata()?.permissions();
    let mut temporary = tempfile::Builder::new()
        .prefix(ARCHIVE_TEMP_PREFIX)
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

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
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
        assert!(!temp.path().join("prod.log.2026-08-09").exists());
        let mut archived = String::new();
        GzDecoder::new(File::open(temp.path().join("prod.log.2026-08-09.gz")).unwrap())
            .read_to_string(&mut archived)
            .unwrap();
        assert_eq!(archived, "old day\n");
    }

    #[test]
    fn explicit_initial_maintenance_compresses_existing_archives() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let archive = temp.path().join("prod.log.2026-08-08");
        fs::write(&archive, "old\n").unwrap();
        let (writer, guard) = DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();

        guard.start_maintenance().unwrap();
        writer.wait_for_maintenance().unwrap();

        assert!(!archive.exists());
        assert!(temp.path().join("prod.log.2026-08-08.gz").exists());
    }

    #[test]
    fn initial_maintenance_removes_abandoned_archive_temporaries() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let abandoned = temp.path().join(format!("{ARCHIVE_TEMP_PREFIX}abandoned"));
        fs::write(&abandoned, "partial archive").unwrap();

        maintain_archives(&active).unwrap();

        assert!(!abandoned.exists());
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
        assert!(!temp.path().join("prod.log.2026-08-01.gz").exists());
        assert!(temp.path().join("prod.log.2026-08-02.gz").exists());
        assert!(temp.path().join("prod.log.2026-08-15.gz").exists());
    }

    #[test]
    fn pruning_continues_when_compression_fails() {
        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let failed_source = temp.path().join("prod.log.2026-08-01");
        fs::create_dir(&failed_source).unwrap();
        for day in 2..=16 {
            fs::write(
                temp.path().join(format!("prod.log.2026-08-{day:02}.gz")),
                "archive\n",
            )
            .unwrap();
        }

        assert!(maintain_archives(&active).is_err());

        assert!(failed_source.exists());
        assert_eq!(find_archives(&active).unwrap().len(), RETAINED_ARCHIVES);
        assert!(!temp.path().join("prod.log.2026-08-02.gz").exists());
        assert!(!temp.path().join("prod.log.2026-08-03.gz").exists());
    }

    #[cfg(unix)]
    #[test]
    fn compressed_archive_preserves_source_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("prod.log.2026-08-09");
        fs::write(&source, "secret\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();

        compress_archive(&source).unwrap();

        let mode = fs::metadata(temp.path().join("prod.log.2026-08-09.gz"))
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
        let source = temp.path().join("prod.log.2026-08-09");
        let victim = temp.path().join("victim");
        fs::write(&source, "archive\n").unwrap();
        fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(&victim, path_with_suffix(&source, ".gz.tmp")).unwrap();

        compress_archive(&source).unwrap();

        assert_eq!(fs::read_to_string(victim).unwrap(), "keep me");
        assert!(path_with_suffix(&source, ".gz").is_file());
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
    fn replacement_log_is_created_with_preserved_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let permissions = fs::Permissions::from_mode(0o600);

        let file = open_append_with_permissions(&active, &permissions).unwrap();

        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn rotation_failure_keeps_appending_to_active_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let active = temp.path().join("prod.log");
        let (mut writer, _guard) =
            DailyRotatingFile::open_with_date(&active, date(2026, 8, 9)).unwrap();
        writer.write_all(b"old\n").unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o500)).unwrap();

        let result = writer.write_on_date(b"new\n", date(2026, 8, 10));

        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(result.unwrap(), 4);
        writer.flush().unwrap();
        assert_eq!(fs::read_to_string(active).unwrap(), "old\nnew\n");
    }

    #[test]
    fn archive_names_do_not_match_unrelated_files() {
        assert!(valid_archive_suffix("2026-08-09"));
        assert!(valid_archive_suffix("2026-08-09-2.gz"));
        assert!(!valid_archive_suffix("0.bz2"));
        assert!(!valid_archive_suffix("2026-08-09.tmp"));
        assert!(!valid_archive_suffix("not-a-date.gz"));
    }
}
