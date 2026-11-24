//! Windows only: the shortcut written and read back through COM in a scratch
//! folder, and the registry reads and deletes under a scratch key of HKCU —
//! never the real Startup folder or `Run` key of the person running the tests.

use super::*;
use windows_sys::Win32::System::Registry::{RegDeleteTreeW, RegSetKeyValueW, REG_BINARY};

/// A key under `HKCU\Software` that exists only for one test, removed on drop.
struct ScratchKey(String);

impl ScratchKey {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        ScratchKey(format!(
            r"Software\Ralphy-test-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[expect(unsafe_code, reason = "FFI: RegSetKeyValueW")]
    fn set(&self, name: &str, bytes: &[u8]) {
        let (key, value) = (wide(&self.0), wide(name));
        let len = u32::try_from(bytes.len()).expect("a small test value");
        // SAFETY: NUL-terminated names and a buffer of `len` bytes.
        let code = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_BINARY,
                bytes.as_ptr().cast(),
                len,
            )
        };
        assert_eq!(code, ERROR_SUCCESS, "writing the scratch value");
    }
}

impl Drop for ScratchKey {
    #[expect(unsafe_code, reason = "FFI: RegDeleteTreeW")]
    fn drop(&mut self) {
        let key = wide(&self.0);
        // SAFETY: a NUL-terminated name of a key this test created.
        let _code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
    }
}

#[test]
fn a_written_shortcut_reads_back_with_its_target_and_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = std::env::current_exe().expect("this test's exe");
    let shortcut = Shortcut {
        link: dir.path().join("Ralphy daemon.lnk"),
        target: target.clone(),
        arguments: r#"--log "C:\a b\daemon.log" daemon"#.to_owned(),
        working_dir: dir.path().to_path_buf(),
        description: "test".to_owned(),
    };
    write_shortcut(&shortcut).expect("writing the shortcut");
    let (read_target, read_arguments) = read_shortcut(&shortcut.link).expect("reading it");
    assert_eq!(read_target, target);
    assert_eq!(read_arguments, shortcut.arguments);
}

#[test]
fn a_registry_value_reads_back_and_deletes_once() {
    let key = ScratchKey::new("value");
    assert_eq!(read_value(&key.0, "ralphy-daemon").expect("reading"), None);
    key.set("ralphy-daemon", &[3, 0, 0, 0]);
    key.set("other", &[1]);
    assert_eq!(
        read_value(&key.0, "ralphy-daemon").expect("reading"),
        Some(vec![3, 0, 0, 0])
    );
    assert!(delete_value(&key.0, "ralphy-daemon").expect("deleting"));
    assert!(!delete_value(&key.0, "ralphy-daemon").expect("deleting again"));
    assert_eq!(
        read_value(&key.0, "other").expect("reading"),
        Some(vec![1]),
        "only its value"
    );
}

#[test]
fn the_startup_folder_is_a_folder_of_this_user() {
    let dir = startup_dir().expect("the Startup folder");
    assert!(
        dir.ends_with(r"Start Menu\Programs\Startup"),
        "{}",
        dir.display()
    );
}
