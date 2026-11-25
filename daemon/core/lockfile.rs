use crate::core::config::default_socket_path;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum LockAcquireError {
    AlreadyRunning { path: PathBuf, pid: Option<u32> },
    Io(String),
}

pub struct InstanceLock {
    path: PathBuf,
    _file: File,
}

impl InstanceLock {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Probes whether another `onehundod` process holds the lockfile.
    /// Returns `Some(pid)` if actively locked, or `None` if free/stale.
    pub fn probe_running(lock_path: &Path) -> Option<Option<u32>> {
        if !lock_path.exists() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;

            const LOCK_EX: i32 = 2;
            const LOCK_NB: i32 = 4;
            const LOCK_UN: i32 = 8;

            extern "C" {
                fn flock(fd: i32, operation: i32) -> i32;
            }

            let mut file = match OpenOptions::new().read(true).open(lock_path) {
                Ok(f) => f,
                Err(_) => return None,
            };
            let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    let mut buf = String::new();
                    let _ = file.read_to_string(&mut buf);
                    let pid = buf.trim().parse::<u32>().ok();
                    return Some(pid);
                }
                return None;
            }
            let _ = unsafe { flock(file.as_raw_fd(), LOCK_UN) };
            None
        }

        #[cfg(not(unix))]
        {
            let pid = fs::read_to_string(lock_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());
            Some(pid)
        }
    }

    pub fn acquire(lock_path: &Path) -> Result<Self, LockAcquireError> {
        if let Some(parent) = lock_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    LockAcquireError::Io(format!(
                        "failed creating lock directory '{}': {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;

            const LOCK_EX: i32 = 2;
            const LOCK_NB: i32 = 4;

            extern "C" {
                fn flock(fd: i32, operation: i32) -> i32;
            }

            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)
                .map_err(|e| {
                    LockAcquireError::Io(format!(
                        "failed opening lockfile '{}': {}",
                        lock_path.display(),
                        e
                    ))
                })?;

            let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    let mut buf = String::new();
                    let _ = file.seek(SeekFrom::Start(0));
                    let _ = file.read_to_string(&mut buf);
                    let pid = buf.trim().parse::<u32>().ok();
                    return Err(LockAcquireError::AlreadyRunning {
                        path: lock_path.to_path_buf(),
                        pid,
                    });
                }
                return Err(LockAcquireError::Io(format!(
                    "failed locking '{}': {}",
                    lock_path.display(),
                    err
                )));
            }

            file.set_len(0).map_err(|e| {
                LockAcquireError::Io(format!("failed truncating lockfile '{}': {}", lock_path.display(), e))
            })?;
            file.seek(SeekFrom::Start(0)).map_err(|e| {
                LockAcquireError::Io(format!("failed seeking lockfile '{}': {}", lock_path.display(), e))
            })?;
            writeln!(file, "{}", std::process::id()).map_err(|e| {
                LockAcquireError::Io(format!("failed writing PID to lockfile '{}': {}", lock_path.display(), e))
            })?;
            let _ = file.flush();

            Ok(Self {
                path: lock_path.to_path_buf(),
                _file: file,
            })
        }

        #[cfg(not(unix))]
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(lock_path)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        let pid = fs::read_to_string(lock_path)
                            .ok()
                            .and_then(|s| s.trim().parse::<u32>().ok());
                        LockAcquireError::AlreadyRunning {
                            path: lock_path.to_path_buf(),
                            pid,
                        }
                    } else {
                        LockAcquireError::Io(format!(
                            "failed creating lockfile '{}': {}",
                            lock_path.display(),
                            e
                        ))
                    }
                })?;
            let _ = writeln!(file, "{}", std::process::id());
            let _ = file.flush();
            Ok(Self {
                path: lock_path.to_path_buf(),
                _file: file,
            })
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn default_lock_path(socket_path: &Path) -> PathBuf {
    if let Ok(p) = env::var("ONEHUNDO_LOCK_FILE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if socket_path == default_socket_path() {
        if let Ok(dir) = env::var("XDG_RUNTIME_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir).join("onehundod.lock");
            }
        }
        return PathBuf::from("/tmp/onehundod.lock");
    }
    socket_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("/tmp"))
        .join("onehundod.lock")
}
