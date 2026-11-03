use std::io;
use std::path::Path;

#[cfg(unix)]
use rustix::fd::OwnedFd;
#[cfg(unix)]
use rustix::fs::{self, AtFlags, Dir, Mode, OFlags};
#[cfg(unix)]
use rustix::io::Errno;

pub(super) struct OwnedPluginData {
    #[cfg(unix)]
    parent: OwnedFd,
    #[cfg(unix)]
    target: OwnedFd,
    #[cfg(unix)]
    name: std::ffi::CString,
}

impl OwnedPluginData {
    #[cfg(unix)]
    pub(super) fn open(root: &Path, name: &super::PluginName<'_>) -> io::Result<Option<Self>> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let root = fs::open(root, flags, Mode::empty())?;
        let parent = match fs::openat(&root, "data", flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let name = std::ffi::CString::new(name.0).map_err(io::Error::other)?;
        let target = match fs::openat(&parent, name.as_c_str(), flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(Self {
            parent,
            target,
            name,
        }))
    }

    #[cfg(not(unix))]
    pub(super) fn open(_root: &Path, _name: &super::PluginName<'_>) -> io::Result<Option<Self>> {
        Err(io::Error::other(
            "no-follow plugin data deletion is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    pub(super) fn remove(self) -> io::Result<()> {
        struct Frame {
            fd: OwnedFd,
            entries: Dir,
            name: std::ffi::CString,
        }
        let Self {
            parent,
            target,
            name,
        } = self;
        let entries = Dir::read_from(&target)?;
        let mut stack = vec![Frame {
            fd: target,
            entries,
            name,
        }];
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        while let Some(frame) = stack.last_mut() {
            if let Some(entry) = frame.entries.next() {
                let entry = entry?;
                let name = entry.file_name();
                if matches!(name.to_bytes(), b"." | b"..") {
                    continue;
                }
                match fs::openat(&frame.fd, name, flags, Mode::empty()) {
                    Ok(fd) => {
                        let entries = Dir::read_from(&fd)?;
                        stack.push(Frame {
                            fd,
                            entries,
                            name: name.to_owned(),
                        });
                    }
                    Err(Errno::NOTDIR | Errno::LOOP) => {
                        match fs::unlinkat(&frame.fd, name, AtFlags::empty()) {
                            Ok(()) | Err(Errno::NOENT) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    Err(Errno::NOENT) => {}
                    Err(error) => return Err(error.into()),
                }
            } else {
                let finished = stack
                    .pop()
                    .ok_or_else(|| io::Error::other("missing deletion frame"))?;
                let parent = stack.last().map_or(&parent, |frame| &frame.fd);
                let expected = fs::fstat(&finished.fd)?;
                let current =
                    fs::statat(parent, finished.name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW)?;
                if (expected.st_dev, expected.st_ino) != (current.st_dev, current.st_ino) {
                    return Err(io::Error::other(
                        "plugin data entry changed during deletion",
                    ));
                }
                fs::unlinkat(parent, finished.name.as_c_str(), AtFlags::REMOVEDIR)?;
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub(super) fn remove(self) -> io::Result<()> {
        Err(io::Error::other(
            "no-follow plugin data deletion is unsupported on this platform",
        ))
    }
}
