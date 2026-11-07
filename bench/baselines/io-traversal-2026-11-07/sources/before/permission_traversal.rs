#![cfg(all(feature = "runtime-fs", unix))]

use napi_vm_runtime::runtime::permissions::Permissions;
use std::{
    os::unix::fs::{FileTypeExt, symlink},
    path::PathBuf,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("napi-vm-traversal-{}-{name}", std::process::id()));
        std::fs::create_dir_all(path.join("root/nested/deep")).unwrap();
        std::fs::create_dir_all(path.join("outside")).unwrap();
        Self(path)
    }
    fn policy(&self) -> Permissions {
        Permissions::new()
            .allow_read(self.0.join("root"))
            .unwrap()
            .allow_write(self.0.join("root"))
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn descriptor_traversal_reads_writes_and_reuses_grants_after_errors() {
    let fixture = Fixture::new("operations");
    let root = fixture.0.join("root");
    let policy = fixture.policy();
    for relative in ["flat", "nested/deep/file"] {
        let path = root.join(relative);
        policy.write_bytes(&path, b"long contents", 100).unwrap();
        policy.write_bytes(&path, b"42", 100).unwrap();
        assert_eq!(policy.read_bytes(&path, 2).unwrap(), b"42");
        assert!(policy.read_bytes(&path, 1).is_err());
        assert!(policy.write_bytes(&path, b"bad", 2).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"42");
    }
    policy.create_dir_all(&root).unwrap();
    policy.create_dir_all(root.join("new/a/b")).unwrap();
    policy.create_dir_all(root.join("new/a/b")).unwrap();
    assert!(root.join("new/a/b").is_dir());
    assert!(policy.read_bytes(&root, 100).is_err());
    assert!(policy.write_bytes(&root, b"bad", 100).is_err());
    for _ in 0..128 {
        assert!(policy.read_bytes(root.join("missing/file"), 100).is_err());
        assert_eq!(policy.read_bytes(root.join("flat"), 100).unwrap(), b"42");
    }
}

#[test]
fn symlinks_aliases_and_special_files_remain_denied() {
    let fixture = Fixture::new("rejections");
    let root = fixture.0.join("root");
    let outside = fixture.0.join("outside");
    std::fs::write(outside.join("secret"), b"unchanged").unwrap();
    symlink(&outside, root.join("link-dir")).unwrap();
    symlink(outside.join("secret"), root.join("link-file")).unwrap();
    let policy = fixture.policy();
    for path in [
        root.join("link-dir/secret"),
        root.join("link-file"),
        root.join("nested/../../outside/secret"),
    ] {
        assert!(policy.read_bytes(&path, 100).is_err());
        assert!(policy.write_bytes(&path, b"bad", 100).is_err());
    }
    assert!(policy.create_dir_all(root.join("link-dir/new")).is_err());
    assert!(!outside.join("new").exists());
    std::fs::hard_link(outside.join("secret"), root.join("hardlink")).unwrap();
    assert!(policy.read_bytes(root.join("hardlink"), 100).is_err());
    assert!(
        policy
            .write_bytes(root.join("hardlink"), b"bad", 100)
            .is_err()
    );
    assert_eq!(std::fs::read(outside.join("secret")).unwrap(), b"unchanged");
    let fifo_path = root.join("fifo");
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(fifo_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: valid NUL-terminated path; the fixture owns and removes the FIFO.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(std::fs::metadata(&fifo_path).unwrap().file_type().is_fifo());
    assert!(policy.read_bytes(&fifo_path, 100).is_err());
    assert!(policy.write_bytes(&fifo_path, b"bad", 100).is_err());
}

#[test]
fn most_specific_grant_stays_anchored_after_directory_replacement() {
    let fixture = Fixture::new("anchor");
    let root = fixture.0.join("root");
    let nested = root.join("nested");
    std::fs::write(nested.join("value"), b"original").unwrap();
    let policy = fixture
        .policy()
        .allow_read(&nested)
        .unwrap()
        .allow_write(&nested)
        .unwrap();
    std::fs::rename(&nested, root.join("moved")).unwrap();
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("value"), b"replacement").unwrap();
    assert_eq!(
        policy.read_bytes(nested.join("value"), 100).unwrap(),
        b"original"
    );
    policy
        .write_bytes(nested.join("value"), b"updated", 100)
        .unwrap();
    policy.create_dir_all(nested.join("created")).unwrap();
    assert_eq!(std::fs::read(root.join("moved/value")).unwrap(), b"updated");
    assert_eq!(std::fs::read(nested.join("value")).unwrap(), b"replacement");
    assert!(root.join("moved/created").is_dir());
    assert!(!nested.join("created").exists());
}
