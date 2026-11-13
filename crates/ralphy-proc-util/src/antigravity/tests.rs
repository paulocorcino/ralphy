use super::*;

use std::path::{Path, PathBuf};

const GUARD_ARGS: [&str; 4] = ["hook", "guard", "--vendor", "antigravity"];
const STATUS_ARGS: [&str; 3] = ["hook", "status", "antigravity"];

/// Write an empty file that the OS would run: on Unix it gets the execute bit.
fn put_program(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The install path the vendor's script writes, for the platform under test.
fn installed_at(home: &Path, localappdata: &Path) -> PathBuf {
    if cfg!(windows) {
        localappdata.join("agy").join("bin").join("agy.exe")
    } else {
        home.join(".local").join("bin").join("agy")
    }
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "agy.exe"
    } else {
        "agy"
    }
}

#[test]
fn locate_agy_prefers_the_install_dir_over_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, localappdata, on_path) = (
        tmp.path().join("home"),
        tmp.path().join("appdata"),
        tmp.path().join("on-path"),
    );
    let path_copy = on_path.join(binary_name());
    put_program(&path_copy);
    let path_var = Some(on_path.clone().into_os_string());
    let pathext = Some(".exe;.cmd".into());

    let found = locate_agy_with(
        path_var.clone(),
        pathext.clone(),
        Some(home.clone()),
        Some(localappdata.clone()),
    );
    assert_eq!(found, Some(path_copy), "with no install, PATH is used");

    let installed = installed_at(&home, &localappdata);
    put_program(&installed);
    let found = locate_agy_with(path_var, pathext, Some(home), Some(localappdata));
    assert_eq!(found, Some(installed));
}

#[test]
fn locate_agy_never_takes_the_ide_launcher() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, localappdata, on_path) = (
        tmp.path().join("home"),
        tmp.path().join("appdata"),
        tmp.path().join("on-path"),
    );
    for launcher in [
        on_path.join("antigravity.cmd"),
        on_path.join("antigravity"),
        localappdata.join("agy").join("bin").join("antigravity.cmd"),
        home.join(".local").join("bin").join("antigravity"),
    ] {
        put_program(&launcher);
    }

    let found = locate_agy_with(
        Some(on_path.into_os_string()),
        Some(".exe;.cmd".into()),
        Some(home),
        Some(localappdata),
    );
    assert_eq!(found, None);
}

#[test]
fn the_owned_root_is_absolute_under_the_ralphy_dir() {
    let home = std::env::temp_dir().join("me");
    let root = owned_root_with(None, Some(home.clone())).unwrap();
    assert_eq!(root, home.join(".ralphy").join("antigravity-root"));

    let root = owned_root_with(Some("relative-store".into()), Some(home)).unwrap();
    assert!(root.is_absolute(), "{}", root.display());
    assert!(
        root.ends_with(Path::new("relative-store").join("antigravity-root")),
        "{}",
        root.display()
    );

    assert!(owned_root_with(None, None).is_err());
}

type Lookup = fn(&Path) -> Option<PathBuf>;

fn no_short_form(path: &Path) -> Option<PathBuf> {
    panic!("a plain path needs no short form: {}", path.display())
}

#[test]
fn an_agy_hook_command_quotes_only_where_the_shell_needs_it() {
    let windows = Path::new(r"C:\Users\me\.local\bin\ralphy.exe");
    assert_eq!(
        hook_command_for(windows, &GUARD_ARGS, HookShell::Cmd, no_short_form).unwrap(),
        r"C:\Users\me\.local\bin\ralphy.exe hook guard --vendor antigravity"
    );

    let unix = Path::new("/home/me/.local/bin/ralphy");
    assert_eq!(
        hook_command_for(unix, &STATUS_ARGS, HookShell::Sh, no_short_form).unwrap(),
        "'/home/me/.local/bin/ralphy' hook status antigravity"
    );
    let odd = Path::new("/home/it's me/$HOME/ralphy");
    assert_eq!(
        hook_command_for(odd, &STATUS_ARGS, HookShell::Sh, no_short_form).unwrap(),
        r"'/home/it'\''s me/$HOME/ralphy' hook status antigravity"
    );

    let err = hook_command_for(unix, &["hook", "a b"], HookShell::Sh, no_short_form);
    assert!(err.is_err(), "an argument that needs quoting is refused");
}

#[test]
fn an_agy_hook_command_with_a_space_uses_the_short_path_or_refuses() {
    let spaced = Path::new(r"C:\Program Files\Ralphy\ralphy.exe");
    let short = |_: &Path| Some(PathBuf::from(r"C:\PROGRA~1\Ralphy\ralphy.exe"));
    assert_eq!(
        hook_command_for(spaced, &STATUS_ARGS, HookShell::Cmd, short).unwrap(),
        r"C:\PROGRA~1\Ralphy\ralphy.exe hook status antigravity"
    );

    let no_short: Lookup = |_| None;
    let same_path: Lookup = |p| Some(p.to_path_buf());
    let ampersand = Path::new(r"C:\Tools&Co\ralphy.exe");
    for (exe, lookup) in [
        (spaced, no_short),
        (spaced, same_path),
        (ampersand, no_short),
    ] {
        let err = hook_command_for(exe, &STATUS_ARGS, HookShell::Cmd, lookup)
            .expect_err("a path cmd cannot run unquoted is refused");
        let text = format!("{err:#}");
        assert!(text.contains(&exe.display().to_string()), "{text}");
    }
}

/// The real lookup on this machine: a short form, when the volume gives one,
/// has no space and names the same file; a refusal still names the path.
#[cfg(windows)]
#[test]
fn the_windows_short_path_lookup_gives_a_form_cmd_can_run() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("with space").join("ralphy.exe");
    put_program(&exe);
    match hook_command(&exe, &STATUS_ARGS) {
        Ok(line) => {
            let program = line.strip_suffix(" hook status antigravity").unwrap();
            assert!(cmd_safe(program), "{line}");
            let same = std::fs::canonicalize(program).unwrap();
            assert_eq!(same, std::fs::canonicalize(&exe).unwrap());
        }
        Err(err) => assert!(format!("{err:#}").contains("with space"), "{err:#}"),
    }
}

#[test]
fn the_antigravity_check_hook_runs_hook_status_without_a_shell() {
    let hook = check_hook(Path::new("ralphy"));
    assert_eq!(hook.form(), crate::hook_form::HookForm::Exec);
    let entry = hook.entry();
    assert_eq!(entry["command"], "ralphy");
    assert_eq!(entry["args"], serde_json::json!(STATUS_ARGS));
}
