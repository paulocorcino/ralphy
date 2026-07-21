//! Building the headless `cursor-agent` invocation: resolving a binary that is on
//! `PATH` on neither platform (ADR-0042 D14), seeding the scratch configuration
//! directory that keeps a run out of the operator's own Cursor state (D17), and
//! fixing the argv that refuses this vendor's default blast radius (D4/D7/D18).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// Mint the session id Ralphy hands the CLI with `--resume`. A v4 UUID: `--resume`
/// with an id that has never existed is accepted silently and echoed back as
/// `system/init.session_id`, so `create-chat` costs a process spawn for nothing
/// (ADR-0042 D10). Adoption is VERIFIED against this value, never assumed.
pub(crate) fn mint_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The model value Ralphy sends when it has no preference. **Never omission**: on
/// this vendor an absent `--model` does not mean "the account default", it means
/// "whatever the last invocation left in `cli-config.json`" (ADR-0042 D4).
pub(crate) const AUTO_MODEL: &str = "auto";

/// The vendor's own name for the two shims it installs for one binary (D14).
const NAMES: [&str; 2] = ["cursor-agent", "agent"];

/// Locate the Cursor CLI. Pure over its inputs so the four install shapes unit-test
/// against temp trees with an empty `PATH` (ADR-0040 C10).
///
/// The order is deliberate: `cursor-agent` is unambiguous, while a bare `agent` on
/// `PATH` could be an unrelated binary, so the specific name and the two known
/// install roots are tried first and `agent` is the last resort.
/// `~/.local/bin/cursor-agent` needs no explicit probe — `locate_program_with`
/// already falls back there.
pub(crate) fn locate_cursor_with(
    path_var: Option<OsString>,
    pathext: Option<OsString>,
    home: Option<PathBuf>,
    localappdata: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(found) = ralphy_adapter_support::locate_program_with(
        NAMES[0],
        path_var.clone(),
        pathext.clone(),
        home.clone(),
    ) {
        return Some(found);
    }
    // `%LOCALAPPDATA%\cursor-agent\` holds `.cmd` + `.ps1` shims for both names.
    if let Some(root) = localappdata.as_ref().map(|p| p.join("cursor-agent")) {
        for name in NAMES {
            let cand = root.join(format!("{name}.cmd"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    // Cursor's own CI recipe names this third location.
    if let Some(bin) = home.as_ref().map(|h| h.join(".cursor").join("bin")) {
        for cand in [bin.join("cursor-agent.cmd"), bin.join("cursor-agent")] {
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    ralphy_adapter_support::locate_program_with(NAMES[1], path_var, pathext, home)
}

/// Locate the Cursor CLI against the real environment. `None` means the vendor is
/// not installed — `ralphy init`'s gate reports presence through this, never
/// through `locate_program("cursor")`, which would look for the wrong binary name.
pub fn locate_cursor() -> Option<PathBuf> {
    locate_cursor_with(
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        ralphy_adapter_support::home_dir(),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    )
}

/// What a `Command` is constructed with. Falls back to the bare name so the spawn
/// failure names the vendor rather than an empty path.
pub(crate) fn resolve_cursor_program() -> OsString {
    locate_cursor()
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| NAMES[0].into())
}

/// The single configuration file that carries the operator's policy (D17). Their
/// `permissions.deny` list lives here, and D7 says that policy is deliberate — so
/// it flows IN to the scratch directory. Nothing flows back.
const CLI_CONFIG: &str = "cli-config.json";

/// Where the operator's own Cursor configuration lives: their explicit
/// `CURSOR_CONFIG_DIR` if they set one (an operator already isolating Cursor is
/// still entitled to their own policy), else `$XDG_CONFIG_HOME/cursor`, else
/// `~/.cursor`.
pub(crate) fn operator_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CURSOR_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("cursor"));
    }
    ralphy_adapter_support::home_dir().map(|h| h.join(".cursor"))
}

/// D17: seed the run's scratch configuration directory from the operator's own.
///
/// `--model` — successful or rejected — rewrites `cli-config.json`'s `model`,
/// `selectedModel` and `modelSelectionHistory` keys, so an unisolated run would
/// reassign the default model of the operator's interactive Cursor sessions. The
/// scratch directory contains that: policy flows in, mutations die with the run.
///
/// **Copies one file, one way.** A missing operator directory or a missing
/// `cli-config.json` is not an error — a fresh install has neither, and the run
/// proceeds against vendor defaults.
pub(crate) fn seed_cursor_config_dir(operator_dir: Option<&Path>, scratch: &Path) -> Result<()> {
    std::fs::create_dir_all(scratch)
        .with_context(|| format!("creating the scratch config dir {}", scratch.display()))?;
    let Some(src) = operator_dir.map(|d| d.join(CLI_CONFIG)) else {
        return Ok(());
    };
    if !src.is_file() {
        return Ok(());
    }
    std::fs::copy(&src, scratch.join(CLI_CONFIG))
        .with_context(|| format!("seeding {} from {}", scratch.display(), src.display()))?;
    Ok(())
}

/// Build the headless `cursor-agent` command both `plan` and `execute` go through.
///
/// The charter is NEVER on argv: `prompt.plan.staged.md` is 25 917 bytes before any
/// issue body against a Windows argv ceiling of ~32 KB, and the spike verified a
/// 26 372-byte payload arriving whole on stdin with markers intact on its first and
/// last line (D2). `-p` here is the vendor's *print mode* switch, which takes no
/// value — there is no prompt word in the argv at all.
///
/// `--model` is ALWAYS present, `auto` when Ralphy has no preference (D4 — see
/// [`AUTO_MODEL`]; omitting it is never correct on this vendor).
/// `--force` is required for non-interactive operation, and the operator's own
/// `permissions.deny` still wins over it (D7).
/// `--output-format stream-json` selects the record stream the fold reads.
/// `--resume` carries Ralphy's minted id, so the session is addressable before the
/// child is spawned (D10).
///
/// The refused flags are refused by ABSENCE, and each is a capability: `--auto-review`
/// (a server-side classifier that prompts — fatal headless — and ships tool-call
/// decisions to a Cursor service), `--approve-mcps` (`.cursor/mcp.json` is
/// repo-local, so a cloned repository could propose servers), `-w`/`--worktree`/
/// `--worktree-base` (Ralphy owns its branches, and `.cursor/worktrees.json`
/// executes repo-local setup scripts), `--mode plan`/`--plan` (hard read-only and
/// it overrides the charter, D9). `--sandbox` is deliberately left unset: forcing a
/// sandbox mode is a capability decision this spike gathered no evidence for.
///
/// Two env vars are set and no more. `CURSOR_CONFIG_DIR` is the D17 containment.
/// `CURSOR_AGENT_DISABLE_DEBUG_LOG` turns off a debug log the CLI writes for every
/// invocation, unasked, into the OS temp directory (D18) — a queue run produces
/// hundreds of invocations and the files name the operator's repositories.
/// `CURSOR_API_KEY`/`CURSOR_AUTH_TOKEN` are left untouched: Ralphy sets neither,
/// and scrubbing them would break an operator who authenticates that way (D8).
pub(crate) fn build_cursor_command(
    session_id: &str,
    model: Option<&str>,
    work_dir: &Path,
    config_dir: &Path,
) -> Command {
    let mut cmd = Command::new(resolve_cursor_program());
    cmd.current_dir(work_dir)
        .arg("-p")
        .arg("--model")
        .arg(model.unwrap_or(AUTO_MODEL))
        .arg("--force")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--resume")
        .arg(session_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CURSOR_CONFIG_DIR", config_dir)
        .env("CURSOR_AGENT_DISABLE_DEBUG_LOG", "1");
    cmd
}

/// The one-shot builder (`init` / `triage` / `consolidate` / `diagnose`).
///
/// Identical argv and environment hygiene to [`build_cursor_command`] — the same
/// D4/D7/D17/D18 stance applies to a one-shot, which walks the same repository the
/// run path does. The only difference is the session id: a one-shot is never
/// resumed and nothing looks it up afterwards, so it gets a fresh minted id rather
/// than one the caller has to thread through.
pub(crate) fn build_cursor_init_command(
    model: Option<&str>,
    cwd: &Path,
    config_dir: &Path,
) -> Command {
    build_cursor_command(&mint_session_id(), model, cwd, config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find_map(|(k, v)| (k == key).then(|| v.map(|v| v.to_string_lossy().into_owned()))?)
    }

    /// D4: `--model` rides EVERY argv, and every flag D7 refuses is absent.
    #[test]
    fn argv_always_carries_a_model_and_never_the_refused_flags() {
        let unpinned = build_cursor_command("s1", None, Path::new("/repo"), Path::new("/run/cfg"));
        let args = argv(&unpinned);
        let i = args
            .iter()
            .position(|a| a == "--model")
            .unwrap_or_else(|| panic!("--model must never be omitted: {args:?}"));
        assert_eq!(args[i + 1], "auto", "argv: {args:?}");

        let pinned = build_cursor_command(
            "s1",
            Some("composer-2.5"),
            Path::new("/repo"),
            Path::new("/run/cfg"),
        );
        let args = argv(&pinned);
        let i = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[i + 1], "composer-2.5", "argv: {args:?}");

        // The rest of the blast radius, refused by absence (D7/D9).
        for cmd in [&unpinned, &pinned] {
            let args = argv(cmd);
            for flag in [
                "--auto-review",
                "--approve-mcps",
                "-w",
                "--worktree",
                "--worktree-base",
                "--sandbox",
                "--mode",
                "--plan",
            ] {
                assert!(
                    !args.iter().any(|a| a == flag),
                    "refused flag {flag} reached argv: {args:?}"
                );
            }
            // The autonomy it does need.
            assert!(args.iter().any(|a| a == "--force"), "argv: {args:?}");
            let i = args.iter().position(|a| a == "--output-format").unwrap();
            assert_eq!(args[i + 1], "stream-json", "argv: {args:?}");
            let i = args.iter().position(|a| a == "--resume").unwrap();
            assert_eq!(args[i + 1], "s1", "argv: {args:?}");
        }
    }

    /// The one-shots inherit the run builder's hygiene wholesale: a `ralphy init`
    /// against a repository is the same blast radius as a run, so a divergence here
    /// would silently exempt four verbs from D4/D7/D17/D18.
    #[test]
    fn the_init_builder_matches_the_run_builders_hygiene() {
        let scratch = Path::new("/run/cfg");
        let cmd = build_cursor_init_command(None, Path::new("/repo"), scratch);
        let args = argv(&cmd);

        let i = args
            .iter()
            .position(|a| a == "--model")
            .unwrap_or_else(|| panic!("--model must never be omitted: {args:?}"));
        assert_eq!(args[i + 1], "auto", "argv: {args:?}");
        assert!(args.iter().any(|a| a == "--force"), "argv: {args:?}");
        let i = args.iter().position(|a| a == "--output-format").unwrap();
        assert_eq!(args[i + 1], "stream-json", "argv: {args:?}");
        for flag in [
            "--auto-review",
            "--approve-mcps",
            "-w",
            "--worktree",
            "--worktree-base",
            "--sandbox",
            "--mode",
            "--plan",
        ] {
            assert!(
                !args.iter().any(|a| a == flag),
                "refused flag {flag} reached a one-shot argv: {args:?}"
            );
        }

        assert_eq!(
            env_of(&cmd, "CURSOR_CONFIG_DIR").map(PathBuf::from),
            Some(scratch.to_path_buf())
        );
        assert_eq!(
            env_of(&cmd, "CURSOR_AGENT_DISABLE_DEBUG_LOG").as_deref(),
            Some("1")
        );

        // The two arguments a builder could silently ignore while still passing
        // every assertion above. `cwd` is D6's premise — the gate is evaluated on
        // the path the CHILD runs in, so a builder that dropped it would gate one
        // directory and index another. `model` dropped would pin every one-shot to
        // `auto` and discard the operator's `--model` on all four verbs.
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/repo")));
        let pinned = build_cursor_init_command(Some("composer-2.5"), Path::new("/repo"), scratch);
        let args = argv(&pinned);
        let i = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[i + 1], "composer-2.5", "argv: {args:?}");
    }

    /// D2: the charter is piped. `-p` is the print-mode switch and takes no value,
    /// so nothing after it may look like prompt text.
    #[test]
    fn argv_carries_no_prompt_word() {
        let cmd = build_cursor_command("s1", None, Path::new("/repo"), Path::new("/run/cfg"));
        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "-p")
            .expect("print mode must be requested");
        assert_eq!(
            args[i + 1],
            "--model",
            "`-p` takes no value — the charter rides stdin: {args:?}"
        );
        // Nothing on the argv is charter-sized prose.
        assert!(
            args.iter().all(|a| a.len() < 64),
            "a prompt-shaped argument reached argv: {args:?}"
        );
    }

    /// D17 + D18: the child never sees the operator's own config dir, and the
    /// vendor's on-by-default debug log is off.
    #[test]
    fn the_child_runs_against_an_isolated_config_dir() {
        let run_dir = Path::new("/run/abc");
        let scratch = run_dir.join("cursor-config");
        let cmd = build_cursor_command("s1", None, Path::new("/repo"), &scratch);

        let got = env_of(&cmd, "CURSOR_CONFIG_DIR").expect("CURSOR_CONFIG_DIR must be set");
        assert_eq!(PathBuf::from(&got), scratch);
        assert!(
            PathBuf::from(&got).starts_with(run_dir),
            "the scratch dir must live under the run dir, got {got}"
        );
        assert!(
            !got.replace('\\', "/").ends_with("/.cursor"),
            "the child must never be pointed at the operator's own dir: {got}"
        );
        assert_eq!(
            env_of(&cmd, "CURSOR_AGENT_DISABLE_DEBUG_LOG").as_deref(),
            Some("1")
        );
        // D8: the credential vars are neither set nor removed.
        for key in ["CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"] {
            assert!(
                !cmd.get_envs().any(|(k, _)| k == key),
                "{key} must be left exactly as the operator has it"
            );
        }
    }

    /// D17's one-way rule, proved by mutating the copy: the operator's file must be
    /// byte-identical afterwards, and their policy must have arrived in the scratch.
    #[test]
    fn seeding_copies_cli_config_in_and_never_back() {
        const POLICY: &str = r#"{"permissions":{"deny":["Shell(git)"]}}"#;
        let operator = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let operator_file = operator.path().join("cli-config.json");
        std::fs::write(&operator_file, POLICY).unwrap();

        seed_cursor_config_dir(Some(operator.path()), scratch.path()).unwrap();

        // D7: the operator's deny list still applies under the isolation.
        let seeded = std::fs::read_to_string(scratch.path().join("cli-config.json")).unwrap();
        assert!(seeded.contains(r#""deny":["Shell(git)"]"#), "{seeded}");

        // The run mutates its copy the way `--model` does.
        std::fs::write(
            scratch.path().join("cli-config.json"),
            r#"{"model":"composer-2.5","hasChangedDefaultModel":true}"#,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&operator_file).unwrap(),
            POLICY,
            "nothing may ever be copied back"
        );
    }

    /// A fresh install has neither directory nor file; that is not an error.
    #[test]
    fn seeding_tolerates_a_missing_operator_config() {
        let scratch = tempfile::tempdir().unwrap();
        let target = scratch.path().join("cursor-config");
        seed_cursor_config_dir(None, &target).unwrap();
        assert!(target.is_dir(), "the scratch dir is created regardless");

        let empty = tempfile::tempdir().unwrap();
        seed_cursor_config_dir(Some(empty.path()), &target).unwrap();
        assert!(!target.join("cli-config.json").exists());
    }

    /// D14: the vendor is on `PATH` on neither platform, under either of its two
    /// names. Each known install shape must resolve with an EMPTY `PATH`.
    #[test]
    fn locate_cursor_finds_each_install_shape() {
        // A file the platform would actually run: on Unix `locate_program_with`
        // requires an execute bit, and on Windows a bare name needs `PATHEXT`.
        fn touch_exe(p: &Path) {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        // 1 & 2: both shim names under %LOCALAPPDATA%\cursor-agent\.
        for name in ["cursor-agent.cmd", "agent.cmd"] {
            let lad = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let want = lad.path().join("cursor-agent").join(name);
            touch_exe(&want);
            let got = locate_cursor_with(
                Some(OsString::new()),
                None,
                Some(home.path().to_path_buf()),
                Some(lad.path().to_path_buf()),
            );
            assert_eq!(got.as_deref(), Some(want.as_path()), "shape: {name}");
        }

        // 3: the XDG shape, reached through `locate_program_with`'s own fallback.
        {
            let home = tempfile::tempdir().unwrap();
            let mut want = home.path().join(".local").join("bin").join("cursor-agent");
            if cfg!(windows) {
                want.set_extension("exe");
            }
            touch_exe(&want);
            let got = locate_cursor_with(
                Some(OsString::new()),
                None,
                Some(home.path().to_path_buf()),
                None,
            );
            assert_eq!(got.as_deref(), Some(want.as_path()), "shape: ~/.local/bin");
        }

        // 4: Cursor's own CI recipe location.
        {
            let home = tempfile::tempdir().unwrap();
            let want = home.path().join(".cursor").join("bin").join("cursor-agent");
            touch_exe(&want);
            let got = locate_cursor_with(
                Some(OsString::new()),
                None,
                Some(home.path().to_path_buf()),
                None,
            );
            assert_eq!(got.as_deref(), Some(want.as_path()), "shape: ~/.cursor/bin");
        }

        // Nothing installed anywhere resolves to nothing — the gate reports absence
        // rather than spawning a name that is not there.
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            locate_cursor_with(Some(OsString::new()), None, Some(home.path().into()), None),
            None
        );
    }

    #[test]
    fn mint_session_id_is_a_fresh_uuid() {
        let a = mint_session_id();
        assert_ne!(a, mint_session_id());
        assert_eq!(a.len(), 36, "not a hyphenated UUID: {a}");
        assert_eq!(a.matches('-').count(), 4, "not a hyphenated UUID: {a}");
    }

    /// ADR-0040 C1: naming the bare binary in a `Command` constructor fails on
    /// Windows for a `.cmd` shim — and on this vendor it fails everywhere, since it
    /// is on `PATH` on neither platform (D14). Fragments are assembled with
    /// `concat!` so this assertion cannot match itself.
    #[test]
    fn no_direct_command_new() {
        // Ban a STRING-LITERAL program name outright: `cursor-agent` and `agent`
        // are both wrong here (neither is on `PATH`), so pinning one spelling would
        // miss the other.
        let production = include_str!("command.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            !production.contains(concat!("Command::", "new(\"")),
            "resolve_cursor_program is the only way to name the binary"
        );
    }
}
