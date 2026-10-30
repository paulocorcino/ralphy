//! The pins of the console family and of the shell family (#621, ADR-0073):
//! the lists of the workbench modules whose text a pin reads, and the tests that
//! keep each list equal to the files on disk. Split from `tests.rs`.

use super::*;

/// The console family (#621, ADR-0073, the 2026-10-11 amendment): every
/// module named `wb-console.ts`, `wb-console-*.ts`, `wb-stage-*.ts`,
/// `wb-desk.ts` or `wb-desk-*.ts`, as `(name, text)`. The order is fixed:
/// `wb-console.ts`, the `wb-console-*` modules, the `wb-stage-*` modules,
/// `wb-desk.ts`, then the `wb-desk-*` modules, each group sorted. A scoped
/// pin finds the first match in the joined text, and `wb-desk-sync.ts`
/// declares a second `function deskOf(`, so the console modules come first.
/// `the_console_family_lists_every_module_on_disk` keeps this list equal to
/// the files on disk.
pub(super) const CONSOLE_FAMILY: &[(&str, &str)] = &[
    (
        "wb-console.ts",
        include_str!("../../assets/ui/wb-console.ts"),
    ),
    (
        "wb-console-api.ts",
        include_str!("../../assets/ui/wb-console-api.ts"),
    ),
    (
        "wb-console-checkout.ts",
        include_str!("../../assets/ui/wb-console-checkout.ts"),
    ),
    (
        "wb-console-dormancy.ts",
        include_str!("../../assets/ui/wb-console-dormancy.ts"),
    ),
    (
        "wb-console-feeds.ts",
        include_str!("../../assets/ui/wb-console-feeds.ts"),
    ),
    (
        "wb-console-fit.ts",
        include_str!("../../assets/ui/wb-console-fit.ts"),
    ),
    (
        "wb-console-frames.ts",
        include_str!("../../assets/ui/wb-console-frames.ts"),
    ),
    (
        "wb-console-gpu.ts",
        include_str!("../../assets/ui/wb-console-gpu.ts"),
    ),
    (
        "wb-console-input.ts",
        include_str!("../../assets/ui/wb-console-input.ts"),
    ),
    (
        "wb-console-keys.ts",
        include_str!("../../assets/ui/wb-console-keys.ts"),
    ),
    (
        "wb-console-name.ts",
        include_str!("../../assets/ui/wb-console-name.ts"),
    ),
    (
        "wb-console-paste.ts",
        include_str!("../../assets/ui/wb-console-paste.ts"),
    ),
    (
        "wb-console-place.ts",
        include_str!("../../assets/ui/wb-console-place.ts"),
    ),
    (
        "wb-console-placeholder.ts",
        include_str!("../../assets/ui/wb-console-placeholder.ts"),
    ),
    (
        "wb-console-press.ts",
        include_str!("../../assets/ui/wb-console-press.ts"),
    ),
    (
        "wb-console-probe.ts",
        include_str!("../../assets/ui/wb-console-probe.ts"),
    ),
    (
        "wb-console-session.ts",
        include_str!("../../assets/ui/wb-console-session.ts"),
    ),
    (
        "wb-console-spawn.ts",
        include_str!("../../assets/ui/wb-console-spawn.ts"),
    ),
    (
        "wb-console-tabs.ts",
        include_str!("../../assets/ui/wb-console-tabs.ts"),
    ),
    (
        "wb-console-terminal-socket.ts",
        include_str!("../../assets/ui/wb-console-terminal-socket.ts"),
    ),
    (
        "wb-console-terminal.ts",
        include_str!("../../assets/ui/wb-console-terminal.ts"),
    ),
    (
        "wb-console-title.ts",
        include_str!("../../assets/ui/wb-console-title.ts"),
    ),
    (
        "wb-console-touch.ts",
        include_str!("../../assets/ui/wb-console-touch.ts"),
    ),
    (
        "wb-stage-chrome.ts",
        include_str!("../../assets/ui/wb-stage-chrome.ts"),
    ),
    (
        "wb-stage-columns.ts",
        include_str!("../../assets/ui/wb-stage-columns.ts"),
    ),
    (
        "wb-stage-fence-gestures.ts",
        include_str!("../../assets/ui/wb-stage-fence-gestures.ts"),
    ),
    (
        "wb-stage-fence-list.ts",
        include_str!("../../assets/ui/wb-stage-fence-list.ts"),
    ),
    (
        "wb-stage-fences.ts",
        include_str!("../../assets/ui/wb-stage-fences.ts"),
    ),
    (
        "wb-stage-parts.ts",
        include_str!("../../assets/ui/wb-stage-parts.ts"),
    ),
    (
        "wb-stage-stack.ts",
        include_str!("../../assets/ui/wb-stage-stack.ts"),
    ),
    (
        "wb-stage-view.ts",
        include_str!("../../assets/ui/wb-stage-view.ts"),
    ),
    (
        "wb-stage-window-gestures.ts",
        include_str!("../../assets/ui/wb-stage-window-gestures.ts"),
    ),
    (
        "wb-stage-window.ts",
        include_str!("../../assets/ui/wb-stage-window.ts"),
    ),
    ("wb-desk.ts", include_str!("../../assets/ui/wb-desk.ts")),
    (
        "wb-desk-detach.ts",
        include_str!("../../assets/ui/wb-desk-detach.ts"),
    ),
    (
        "wb-desk-folds.ts",
        include_str!("../../assets/ui/wb-desk-folds.ts"),
    ),
    (
        "wb-desk-history.ts",
        include_str!("../../assets/ui/wb-desk-history.ts"),
    ),
    (
        "wb-desk-popups.ts",
        include_str!("../../assets/ui/wb-desk-popups.ts"),
    ),
    (
        "wb-desk-records.ts",
        include_str!("../../assets/ui/wb-desk-records.ts"),
    ),
    (
        "wb-desk-restore.ts",
        include_str!("../../assets/ui/wb-desk-restore.ts"),
    ),
    (
        "wb-desk-sink.ts",
        include_str!("../../assets/ui/wb-desk-sink.ts"),
    ),
    (
        "wb-desk-sync.ts",
        include_str!("../../assets/ui/wb-desk-sync.ts"),
    ),
];

/// The texts of `CONSOLE_FAMILY`, joined in its order. A pin on console text
/// reads this, so a move between family modules does not move the pin
/// (#621).
pub(super) fn console_family() -> String {
    CONSOLE_FAMILY.iter().map(|(_, text)| *text).collect()
}

/// The group of a console family module in the order of `CONSOLE_FAMILY`, or
/// `None` for a module outside the family. `wb-consoles-tab.ts` is outside:
/// the rule needs the hyphen after `wb-console`.
fn console_family_group(name: &str) -> Option<u8> {
    if !name.ends_with(".ts") || name.ends_with(".d.ts") {
        return None;
    }
    if name == "wb-console.ts" {
        Some(0)
    } else if name.starts_with("wb-console-") {
        Some(1)
    } else if name.starts_with("wb-stage-") {
        Some(2)
    } else if name == "wb-desk.ts" {
        Some(3)
    } else if name.starts_with("wb-desk-") {
        Some(4)
    } else {
        None
    }
}

/// #621: a new console family module cannot be left out of the pins that
/// read the family, because this test compares the list with the disk.
#[test]
fn the_console_family_lists_every_module_on_disk() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut on_disk: Vec<(u8, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter_map(|name| console_family_group(&name).map(|group| (group, name)))
        .collect();
    on_disk.sort();
    let on_disk: Vec<&str> = on_disk.iter().map(|(_, name)| name.as_str()).collect();
    let listed: Vec<&str> = CONSOLE_FAMILY.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        listed, on_disk,
        "CONSOLE_FAMILY in crates/ralphy-daemon/src/tests/families.rs must list every console family \
         module under assets/ui, in its order: add the new module to CONSOLE_FAMILY"
    );
}

/// The shell family (#621, ADR-0073, the amendment of 2026-10-11): `app.ts`,
/// then `wb-wire.ts` and `wb-icon.ts` once they exist, then every module that
/// `main.ts` imports a registered Alpine component or Alpine store from,
/// sorted, as `(name, text)`. `app.ts` comes first: a scoped pin finds the
/// first match in the joined text, and every slice the pins take was measured
/// inside `app.ts` when the family was made.
/// `the_shell_family_lists_every_module_on_disk` keeps this list equal to
/// `main.ts`.
pub(super) const SHELL_FAMILY: &[(&str, &str)] = &[
    ("app.ts", include_str!("../../assets/ui/app.ts")),
    ("wb-wire.ts", include_str!("../../assets/ui/wb-wire.ts")),
    ("wb-icon.ts", include_str!("../../assets/ui/wb-icon.ts")),
    (
        "wb-account.ts",
        include_str!("../../assets/ui/wb-account.ts"),
    ),
    (
        "wb-add-project-dialog.ts",
        include_str!("../../assets/ui/wb-add-project-dialog.ts"),
    ),
    (
        "wb-attention.ts",
        include_str!("../../assets/ui/wb-attention.ts"),
    ),
    (
        "wb-board-store.ts",
        include_str!("../../assets/ui/wb-board-store.ts"),
    ),
    ("wb-board.ts", include_str!("../../assets/ui/wb-board.ts")),
    (
        "wb-branch-dialog.ts",
        include_str!("../../assets/ui/wb-branch-dialog.ts"),
    ),
    (
        "wb-changes-panel.ts",
        include_str!("../../assets/ui/wb-changes-panel.ts"),
    ),
    (
        "wb-checkouts-store.ts",
        include_str!("../../assets/ui/wb-checkouts-store.ts"),
    ),
    (
        "wb-connection-store.ts",
        include_str!("../../assets/ui/wb-connection-store.ts"),
    ),
    (
        "wb-console-tabs.ts",
        include_str!("../../assets/ui/wb-console-tabs.ts"),
    ),
    (
        "wb-consoles-tab-menus.ts",
        include_str!("../../assets/ui/wb-consoles-tab-menus.ts"),
    ),
    (
        "wb-consoles-tab.ts",
        include_str!("../../assets/ui/wb-consoles-tab.ts"),
    ),
    (
        "wb-devices.ts",
        include_str!("../../assets/ui/wb-devices.ts"),
    ),
    (
        "wb-files-upload.ts",
        include_str!("../../assets/ui/wb-files-upload.ts"),
    ),
    ("wb-files.ts", include_str!("../../assets/ui/wb-files.ts")),
    (
        "wb-hosts-dialog.ts",
        include_str!("../../assets/ui/wb-hosts-dialog.ts"),
    ),
    ("wb-login.ts", include_str!("../../assets/ui/wb-login.ts")),
    (
        "wb-modals-store.ts",
        include_str!("../../assets/ui/wb-modals-store.ts"),
    ),
    (
        "wb-move-dialog.ts",
        include_str!("../../assets/ui/wb-move-dialog.ts"),
    ),
    (
        "wb-projects-store.ts",
        include_str!("../../assets/ui/wb-projects-store.ts"),
    ),
    (
        "wb-properties-dialog.ts",
        include_str!("../../assets/ui/wb-properties-dialog.ts"),
    ),
    (
        "wb-release-dialogs.ts",
        include_str!("../../assets/ui/wb-release-dialogs.ts"),
    ),
    (
        "wb-release-store.ts",
        include_str!("../../assets/ui/wb-release-store.ts"),
    ),
    (
        "wb-runs-panel.ts",
        include_str!("../../assets/ui/wb-runs-panel.ts"),
    ),
    (
        "wb-runs-store.ts",
        include_str!("../../assets/ui/wb-runs-store.ts"),
    ),
    (
        "wb-security-dialog.ts",
        include_str!("../../assets/ui/wb-security-dialog.ts"),
    ),
    (
        "wb-settings-dialog.ts",
        include_str!("../../assets/ui/wb-settings-dialog.ts"),
    ),
    (
        "wb-spend-panel.ts",
        include_str!("../../assets/ui/wb-spend-panel.ts"),
    ),
    (
        "wb-tabs-store.ts",
        include_str!("../../assets/ui/wb-tabs-store.ts"),
    ),
    (
        "wb-uploads-store.ts",
        include_str!("../../assets/ui/wb-uploads-store.ts"),
    ),
];

/// The texts of `SHELL_FAMILY`, joined in its order. A pin on shell text reads
/// this, so a move from `app.ts` to a component or a store of the family does
/// not move the pin (#621).
pub(super) fn shell_family() -> String {
    SHELL_FAMILY.iter().map(|(_, text)| *text).collect()
}

/// #621: a module that `main.ts` registers with Alpine cannot be left out of
/// the pins that read the shell family, because this test builds the list
/// from `main.ts` and the disk.
#[test]
fn the_shell_family_lists_every_module_on_disk() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let main_path = dir.join("main.ts");
    let main = std::fs::read_to_string(&main_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", main_path.display()));
    // The module of each name a one-line import of `main.ts` gives
    // (`static_imports` reads only one-line imports too).
    let import =
        regex::Regex::new(r#"(?m)^import \{([^}]*)\} from "\./([^"]+)";"#).expect("valid regex");
    let mut module_of = std::collections::HashMap::new();
    for cap in import.captures_iter(&main) {
        for part in cap[1].split(',') {
            if let Some(name) = part.split_whitespace().last() {
                module_of.insert(name.to_string(), cap[2].to_string());
            }
        }
    }
    let register =
        regex::Regex::new(r#"Alpine\.(?:data|store)\(\s*"[^"]+"\s*,\s*([A-Za-z_$][\w$]*)"#)
            .expect("valid regex");
    let mut registered = std::collections::BTreeSet::new();
    for cap in register.captures_iter(&main) {
        let factory = &cap[1];
        let module = module_of.get(factory).unwrap_or_else(|| {
            panic!("main.ts registers {factory}, and no one-line import of main.ts gives it")
        });
        registered.insert(module.clone());
    }
    assert!(
        registered.contains("app.ts"),
        "main.ts must register the shell from app.ts"
    );
    let mut expected = vec!["app.ts".to_string()];
    for front in ["wb-wire.ts", "wb-icon.ts"] {
        if dir.join(front).is_file() {
            expected.push(front.to_string());
        }
    }
    let rest: Vec<String> = registered
        .into_iter()
        .filter(|module| !expected.contains(module))
        .collect();
    expected.extend(rest);
    let listed: Vec<&str> = SHELL_FAMILY.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        listed, expected,
        "SHELL_FAMILY in crates/ralphy-daemon/src/tests/families.rs must list app.ts, then wb-wire.ts \
         and wb-icon.ts when they exist, then every module main.ts registers an Alpine \
         component or Alpine store from, sorted: add the module to SHELL_FAMILY"
    );
    for (name, text) in SHELL_FAMILY {
        let path = dir.join(name);
        let on_disk = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        assert_eq!(
            *text, on_disk,
            "the SHELL_FAMILY entry {name} must include the text of {name}"
        );
    }
}
