//! Dynamic loading of the host Node-API shim library.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak as SyncWeak};

#[cfg(unix)]
use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
#[cfg(target_os = "windows")]
use libloading::os::windows::{
    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    LOAD_LIBRARY_SEARCH_USER_DIRS, Library,
};
#[cfg(target_os = "windows")]
use std::collections::HashSet;
#[cfg(target_os = "windows")]
use std::ffi::{OsStr, c_void};

use crate::error::VmErr;

use super::api_table::{NAPI_VM_API_TABLE, NapiVmApiTable};
#[cfg(target_os = "windows")]
use super::{
    AddDllDirectory, GetModuleHandleW, RemoveDllDirectory, WINDOWS_NODE_API_SHIM_DIRECTORIES,
};
use super::{
    PINNED_NODE_API_ADDON_LIBRARIES, PROCESS_NODE_API_ADDON_LIBRARIES, PROCESS_NODE_API_SHIM,
};

pub(super) struct NodeApiShim {
    pub(super) _library: Option<Library>,
    pub(super) path: PathBuf,
    #[cfg(target_os = "windows")]
    pub(super) dll_directory_cookie: usize,
}

impl NodeApiShim {
    pub(super) fn load() -> Result<Arc<Self>, VmErr> {
        let process_shim = PROCESS_NODE_API_SHIM.get_or_init(|| Mutex::new(None));
        let mut process_shim = process_shim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(shim) = process_shim.as_ref() {
            return Ok(shim.clone());
        }
        let shim = Arc::new(Self::load_uncached()?);
        *process_shim = Some(shim.clone());
        Ok(shim)
    }

    pub(super) fn load_uncached() -> Result<Self, VmErr> {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let bytes = include_bytes!(env!("NAPI_VM_NODE_API_SHIM_PATH"));
        let root = loop {
            let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir()
                .join(format!("napi-vm-node-api-{}-{nonce}", std::process::id()));
            #[cfg(unix)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(VmErr::Msg(format!(
                        "cannot create private Node-API shim directory: {error}"
                    )));
                }
            }
        };
        #[cfg(target_os = "linux")]
        let path = root.join("libnapi_vm_node_api_shim.so");
        #[cfg(target_os = "macos")]
        let path = root.join("libnapi_vm_node_api_shim.dylib");
        #[cfg(target_os = "windows")]
        let path = root.join("node.exe");
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o500))?;
            }
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_dir_all(&root);
            return Err(VmErr::Msg(format!(
                "cannot materialize Node-API symbol shim: {error}"
            )));
        }

        #[cfg(target_os = "windows")]
        let dll_directory_cookie = {
            use std::os::windows::ffi::OsStrExt;
            let mut directory: Vec<u16> = root.as_os_str().encode_wide().collect();
            directory.push(0);
            let cookie = unsafe { AddDllDirectory(directory.as_ptr()) };
            if cookie.is_null() {
                let error = std::io::Error::last_os_error();
                let _ = fs::remove_dir_all(&root);
                return Err(VmErr::Msg(format!(
                    "cannot register private Node-API shim directory: {error}"
                )));
            }
            cookie as usize
        };

        #[cfg(unix)]
        let library_result =
            unsafe { Library::open(Some(path.as_os_str()), RTLD_NOW | RTLD_GLOBAL) };
        #[cfg(target_os = "windows")]
        let library_result = unsafe { Library::new(path.as_os_str()) };
        let library = library_result.map_err(|error| {
            #[cfg(target_os = "windows")]
            unsafe {
                let _ = RemoveDllDirectory(dll_directory_cookie as *mut c_void);
            }
            {
                let _ = fs::remove_dir_all(&root);
                VmErr::Msg(format!("cannot load Node-API symbol shim: {error}"))
            }
        })?;
        let install: unsafe extern "C" fn(*const NapiVmApiTable) =
            match unsafe { library.get(b"napi_vm_install_node_api_table\0") } {
                Ok(symbol) => *symbol,
                Err(error) => {
                    drop(library);
                    #[cfg(target_os = "windows")]
                    unsafe {
                        let _ = RemoveDllDirectory(dll_directory_cookie as *mut c_void);
                    }
                    let _ = fs::remove_dir_all(&root);
                    return Err(VmErr::Msg(format!("invalid Node-API symbol shim: {error}")));
                }
            };
        unsafe { install(&NAPI_VM_API_TABLE) };
        #[cfg(unix)]
        {
            // The process singleton keeps the mapping live. Unix permits
            // unlinking a mapped shared object, so do not leave a temp copy
            // behind for each application run.
            let _ = fs::remove_dir_all(&root);
        }
        #[cfg(target_os = "windows")]
        WINDOWS_NODE_API_SHIM_DIRECTORIES
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(root.clone());
        Ok(Self {
            _library: Some(library),
            path,
            #[cfg(target_os = "windows")]
            dll_directory_cookie,
        })
    }

    pub(super) fn load_addon(&self, filename: &Path) -> Result<Arc<Library>, libloading::Error> {
        let key = filename.to_path_buf();
        if let Some(library) = PINNED_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned()
        {
            return Ok(library);
        }
        if let Some(library) = PROCESS_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .and_then(SyncWeak::upgrade)
        {
            return Ok(library);
        }

        let library = Arc::new(self.open_addon(filename)?);
        let pinned = PINNED_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(library) = pinned.get(&key) {
            return Ok(library.clone());
        }
        let mut cache = PROCESS_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|_, library| library.strong_count() != 0);
        if let Some(library) = cache.get(&key).and_then(SyncWeak::upgrade) {
            return Ok(library);
        }
        cache.insert(key, Arc::downgrade(&library));
        Ok(library)
    }

    pub(super) fn open_addon(&self, filename: &Path) -> Result<Library, libloading::Error> {
        #[cfg(unix)]
        {
            unsafe { Library::open(Some(filename.as_os_str()), RTLD_NOW) }
        }
        #[cfg(target_os = "windows")]
        {
            unsafe {
                Library::load_with_flags(
                    filename.as_os_str(),
                    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
                        | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                        | LOAD_LIBRARY_SEARCH_USER_DIRS,
                )
            }
        }
    }
}

impl Drop for NodeApiShim {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::ffi::OsStrExt;

            // Addon library handles are stored before the shim in HostState,
            // so they have closed before the Node-API import provider unloads.
            drop(self._library.take());
            unsafe {
                let _ = RemoveDllDirectory(self.dll_directory_cookie as *mut c_void);
            }
            let mut module_name: Vec<u16> = OsStr::new("node.exe").encode_wide().collect();
            module_name.push(0);
            if unsafe { GetModuleHandleW(module_name.as_ptr()) }.is_null()
                && let Some(directories) = WINDOWS_NODE_API_SHIM_DIRECTORIES.get()
            {
                let mut directories = directories
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut roots: HashSet<PathBuf> = directories.drain(..).collect();
                if let Some(root) = self.path.parent() {
                    roots.insert(root.to_path_buf());
                }
                for root in roots {
                    let _ = fs::remove_dir_all(root);
                }
            }
        }
        #[cfg(unix)]
        {
            // Unix permits unlinking a loaded shared object; the mapping
            // remains live until the Library is dropped immediately after this method.
            if let Some(root) = self.path.parent() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }
}
