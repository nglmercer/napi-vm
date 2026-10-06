//! Default-denied runtime permissions. File grants are anchored to directory
//! descriptors on Unix; symlinks are rejected during traversal.
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;

use crate::VmErr;

#[derive(Clone, Debug)]
struct DirectoryGrant {
    path: PathBuf,
    #[cfg(unix)]
    directory: Arc<File>,
}

/// Availability of an API never grants permission to use it.
#[derive(Clone, Debug, Default)]
pub struct Permissions {
    read: Vec<DirectoryGrant>,
    write: Vec<DirectoryGrant>,
    net: HashSet<(String, Option<u16>)>,
    env: HashSet<String>,
    process: bool,
    ffi: bool,
}

fn denied(capability: &str) -> VmErr {
    VmErr::Msg(format!(
        "PermissionDenied: {capability} access is not allowed"
    ))
}

impl Permissions {
    pub fn new() -> Self {
        Self::default()
    }
    /// Combine explicitly supplied grants, retaining default denial elsewhere.
    pub fn merge(mut self, other: Self) -> Self {
        self.read.extend(other.read);
        self.write.extend(other.write);
        self.net.extend(other.net);
        self.env.extend(other.env);
        self.process |= other.process;
        self.ffi |= other.ffi;
        self
    }

    pub fn allow_read(mut self, directory: impl AsRef<Path>) -> std::io::Result<Self> {
        self.read.push(Self::directory(directory.as_ref())?);
        Ok(self)
    }
    pub fn allow_write(mut self, directory: impl AsRef<Path>) -> std::io::Result<Self> {
        self.write.push(Self::directory(directory.as_ref())?);
        Ok(self)
    }
    fn directory(path: &Path) -> std::io::Result<DirectoryGrant> {
        let path = path.canonicalize()?;
        if !path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "permission root must be a directory",
            ));
        }
        #[cfg(unix)]
        let directory = {
            use std::os::unix::fs::OpenOptionsExt;
            Arc::new(
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&path)?,
            )
        };
        Ok(DirectoryGrant {
            path,
            #[cfg(unix)]
            directory,
        })
    }
    /// Exact hostname, optionally constrained to a port. Wildcards are not grants.
    pub fn allow_net(mut self, host: impl Into<String>, port: Option<u16>) -> Self {
        self.net.insert((host.into().to_ascii_lowercase(), port));
        self
    }
    pub fn allow_env(mut self, name: impl Into<String>) -> Self {
        self.env.insert(name.into());
        self
    }
    pub fn allow_process(mut self) -> Self {
        self.process = true;
        self
    }
    pub fn allow_ffi(mut self) -> Self {
        self.ffi = true;
        self
    }
    pub fn check_net(&self, host: &str, port: u16) -> Result<(), VmErr> {
        let host = host.to_ascii_lowercase();
        if self.net.contains(&(host.clone(), Some(port))) || self.net.contains(&(host, None)) {
            Ok(())
        } else {
            Err(denied("network"))
        }
    }
    pub fn check_process(&self) -> Result<(), VmErr> {
        if self.process {
            Ok(())
        } else {
            Err(denied("process"))
        }
    }
    pub fn check_ffi(&self) -> Result<(), VmErr> {
        if self.ffi { Ok(()) } else { Err(denied("FFI")) }
    }
    pub fn environment(&self, name: &str) -> Result<Option<String>, VmErr> {
        if !self.env.contains(name) {
            return Err(denied("environment"));
        }
        match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(VmErr::Msg(error.to_string())),
        }
    }
    pub fn read_text(&self, path: impl AsRef<Path>, max_bytes: usize) -> Result<String, VmErr> {
        String::from_utf8(self.read_bytes(path, max_bytes)?).map_err(|e| VmErr::Msg(e.to_string()))
    }
    pub fn read_bytes(&self, path: impl AsRef<Path>, max_bytes: usize) -> Result<Vec<u8>, VmErr> {
        let file = self.open(path.as_ref(), false)?;
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| VmErr::Msg(e.to_string()))?;
        if bytes.len() > max_bytes {
            return Err(VmErr::Msg("ResourceLimit: file size exceeded".into()));
        }
        Ok(bytes)
    }
    pub fn write_text(
        &self,
        path: impl AsRef<Path>,
        text: &str,
        max_bytes: usize,
    ) -> Result<(), VmErr> {
        self.write_bytes(path, text.as_bytes(), max_bytes)
    }
    pub fn write_bytes(
        &self,
        path: impl AsRef<Path>,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<(), VmErr> {
        if bytes.len() > max_bytes {
            return Err(VmErr::Msg("ResourceLimit: file size exceeded".into()));
        }
        let mut file = self.open(path.as_ref(), true)?;
        // Never truncate before checking that this is a regular, non-aliased file.
        file.set_len(0).map_err(|e| VmErr::Msg(e.to_string()))?;
        file.write_all(bytes).map_err(|e| VmErr::Msg(e.to_string()))
    }
    /// Create directories through granted directory descriptors; never follow symlinks.
    pub fn create_dir_all(&self, path: impl AsRef<Path>) -> Result<(), VmErr> {
        let path = if path.as_ref().is_absolute() {
            path.as_ref().to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| VmErr::Msg(e.to_string()))?
                .join(path)
        };
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(denied("filesystem"));
        }
        let (grant, relative) = self
            .write
            .iter()
            .filter_map(|g| path.strip_prefix(&g.path).ok().map(|p| (g, p)))
            .max_by_key(|(g, _)| g.path.components().count())
            .ok_or_else(|| denied("filesystem"))?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd, FromRawFd};
            use std::os::unix::ffi::OsStrExt;
            let mut directory = grant
                .directory
                .try_clone()
                .map_err(|e| VmErr::Msg(e.to_string()))?;
            for name in relative.components().filter_map(|c| {
                if let Component::Normal(n) = c {
                    Some(n)
                } else {
                    None
                }
            }) {
                let name = CString::new(name.as_bytes()).map_err(|_| denied("filesystem"))?;
                // SAFETY: live descriptor and valid NUL-terminated component. The
                // created directory is immediately reopened without following links.
                let status = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
                if status != 0
                    && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
                {
                    return Err(VmErr::Msg(std::io::Error::last_os_error().to_string()));
                }
                let fd = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_RDONLY | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    return Err(VmErr::Msg(std::io::Error::last_os_error().to_string()));
                }
                directory = unsafe { File::from_raw_fd(fd) };
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (grant, relative);
            Err(VmErr::Msg(
                "Unsupported: secure filesystem traversal requires Unix".into(),
            ))
        }
    }
    fn open(&self, path: &Path, write: bool) -> Result<File, VmErr> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| VmErr::Msg(e.to_string()))?
                .join(path)
        };
        // Parent components are rejected rather than normalized across symlinks.
        if absolute
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(denied("filesystem"));
        }
        let grants = if write { &self.write } else { &self.read };
        let (grant, relative) = grants
            .iter()
            .filter_map(|g| absolute.strip_prefix(&g.path).ok().map(|p| (g, p)))
            .max_by_key(|(g, _)| g.path.components().count())
            .ok_or_else(|| denied("filesystem"))?;
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd, FromRawFd};
            use std::os::unix::ffi::OsStrExt;
            use std::os::unix::fs::MetadataExt;
            let names: Vec<_> = relative
                .components()
                .filter_map(|c| match c {
                    Component::Normal(n) => Some(n),
                    _ => None,
                })
                .collect();
            if names.is_empty() {
                return Err(denied("filesystem"));
            }
            let mut directory = grant
                .directory
                .try_clone()
                .map_err(|e| VmErr::Msg(e.to_string()))?;
            for (index, name) in names.iter().enumerate() {
                let name = CString::new(name.as_bytes()).map_err(|_| denied("filesystem"))?;
                let last = index + 1 == names.len();
                let flags = libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK
                    | if !last {
                        libc::O_RDONLY | libc::O_DIRECTORY
                    } else if write {
                        libc::O_WRONLY | libc::O_CREAT
                    } else {
                        libc::O_RDONLY
                    };
                // SAFETY: live directory descriptor, valid NUL-terminated name;
                // a successful fd is immediately owned by File.
                let fd =
                    unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
                if fd < 0 {
                    return Err(VmErr::Msg(std::io::Error::last_os_error().to_string()));
                }
                let file = unsafe { File::from_raw_fd(fd) };
                if last {
                    let meta = file.metadata().map_err(|e| VmErr::Msg(e.to_string()))?;
                    if !meta.is_file() || meta.nlink() != 1 {
                        return Err(denied("filesystem"));
                    }
                    return Ok(file);
                }
                directory = file;
            }
            Err(denied("filesystem"))
        }
        #[cfg(not(unix))]
        {
            let _ = (grant, relative);
            Err(VmErr::Msg(
                "Unsupported: secure filesystem traversal requires Unix".into(),
            ))
        }
    }
}
