//! What to launch: the vendor-neutral [`SessionSpec`], the [`Agent`] roster
//! and the per-vendor/console launch specs resolved from a repo.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A program-neutral description of what to launch inside the PTY. Program +
/// args are already resolved (no agent knowledge), so `Session::spawn` is
/// testable against any helper bin.
pub struct SessionSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub rows: u16,
    pub cols: u16,
    /// Extra environment for the child, applied on the spawn path ONLY
    /// ([`super::Session::spawn`]). A vendor whose containment lives in an env var —
    /// Gemini's `GEMINI_CLI_HOME` (ADR-0043 D4) — is isolated by this and nothing
    /// else, so no other construction site may skip it.
    pub env: Vec<(OsString, OsString)>,
    /// The display name this child announces itself under, when the vendor has
    /// somewhere to announce it. An OPAQUE label here — which flag carries it is
    /// [`spec_for`]'s business, so this struct stays program-neutral — and the
    /// same string is copied onto [`super::SessionInfo`] so the shell can show the
    /// operator the name other sessions address this console by.
    pub name: Option<String>,
    /// The agent-state files this console's hooks use (ADR-0059 §5), when
    /// the vendor has hooks and the daemon wrote them: the session tails the
    /// status file and removes both with the session. `None` for every other
    /// child.
    pub status: Option<crate::agent_state::StatusFiles>,
}

/// The agents the launcher can start. Maps to a concrete program via
/// [`spec_for`]; the bare interactive launch (no extra args) is what opens each
/// vendor's TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Copilot,
    Cursor,
    Gemini,
    Kimi,
    OpenCode,
}

impl Agent {
    /// Every launchable vendor. The anti-drift tests (the workbench trio, the
    /// usage-store resolvers) enumerate the daemon's vendors from here, so an
    /// eighth variant reds them instead of passing silently.
    pub const ALL: [Agent; 7] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Copilot,
        Agent::Cursor,
        Agent::Gemini,
        Agent::Kimi,
        Agent::OpenCode,
    ];

    /// Parse the `agent=` query value. Unknown values yield `None` so the route
    /// can reject them rather than launching a surprise program.
    pub fn from_query(value: &str) -> Option<Agent> {
        match value {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "copilot" => Some(Agent::Copilot),
            "cursor" => Some(Agent::Cursor),
            "gemini" => Some(Agent::Gemini),
            "kimi" => Some(Agent::Kimi),
            "opencode" => Some(Agent::OpenCode),
            _ => None,
        }
    }

    /// The program name to resolve on `PATH` for this agent.
    fn program_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            // The GitHub Copilot CLI ships as `copilot`, the same name the
            // adapter resolves for its headless calls (ADR-0041).
            Agent::Copilot => "copilot",
            // Cursor ships one binary under two names (`cursor-agent`, `agent`)
            // across three install roots and is on `PATH` under NEITHER
            // (ADR-0042 D14) — so this name is only the fallback that makes a
            // spawn failure legible; [`Agent::resolve_program`] does the real work.
            Agent::Cursor => "cursor-agent",
            // npm installs the Gemini CLI as `gemini` (+ a `.CMD`/`.ps1` shim on
            // Windows); the shared resolver already picks the `.CMD` (#253), so
            // this vendor needs no bespoke locator.
            Agent::Gemini => "gemini",
            // `kimi-code` ships its binary as `kimi` — the same name the adapter
            // resolves for its headless calls (ADR-0028 D5).
            Agent::Kimi => "kimi",
            Agent::OpenCode => "opencode",
        }
    }

    /// What a `Command` should be constructed with. Every vendor but Cursor is
    /// found on `PATH` (plus the `~/.local/bin` fallback); Cursor needs its own
    /// locator, shared with the run path so detection and execution cannot
    /// disagree (ADR-0042 D14/D19).
    pub fn locate_program(self) -> Option<PathBuf> {
        match self {
            Agent::Cursor => ralphy_proc_util::cursor::locate_cursor(),
            _ => ralphy_proc_util::locate_program(self.program_name()),
        }
    }

    fn resolve_program(self) -> OsString {
        self.locate_program()
            .map(PathBuf::into_os_string)
            .unwrap_or_else(|| self.program_name().into())
    }
}

/// Environment override pointing the launcher at a stand-in program (the test
/// helper bin). A test-only seam — inert in production — because an integration
/// test's binary is not reachable from a `#[cfg(test)]` path in the compiled lib.
const AGENT_OVERRIDE_ENV: &str = "RALPHY_DAEMON_AGENT_OVERRIDE";

/// Build the launch spec for `agent` at the given terminal size. The program
/// is resolved through `ralphy_proc_util::resolve_program` (Windows
/// `.cmd`/`.exe` shims included), unless `RALPHY_DAEMON_AGENT_OVERRIDE` names a
/// program to run instead.
///
/// `root` is the primary tree — every `.ralphy/`-backed read (Gemini's owned
/// home and policy, Claude's name opt-in, Cursor's indexing opt-in) resolves
/// there, because a checkout has no `.ralphy/`; `cwd` is where the child runs:
/// `root` itself, or a checkout's directory (ADR-0063 §3). No vendor is ever
/// handed a worktree flag: the worktree is Ralphy's and arrives as `cwd`.
///
/// Gemini alone carries args and env: an interactive launch must land in the SAME
/// owned configuration root and under the SAME policy document a `ralphy run`
/// uses (ADR-0043 D4/D6), which is `GEMINI_CLI_HOME` plus the global `--policy`
/// flag. The route refuses the launch when that document is absent, so this
/// function never has to invent one.
///
/// Claude alone can carry a name: its sessions address each other by display
/// name (`SendMessage({to: "<name>"})`), and the name it picks for itself is
/// `<folder>-<2 hex>` — indistinguishable, in a roster that spans the whole
/// machine, from the other consoles open on the same repo. [`console_name`]
/// replaces it with one that says which repo AND that a workbench opened it. No
/// other vendor has an equivalent flag.
///
/// The name is OPT-IN and off by default, read per repo from
/// [`claude_console_named`]: renaming a session changes the address every other
/// session already knows it by, so it is something the operator asks for. Off,
/// this function passes no `--name` and leaves `spec.name` `None` — the vendor
/// names the session itself and the shell shows what it announced.
pub fn spec_for(
    agent: Agent,
    root: &Path,
    cwd: PathBuf,
    repo_slug: &str,
    rows: u16,
    cols: u16,
) -> SessionSpec {
    spec_with_status(agent, root, cwd, repo_slug, rows, cols, None)
}

/// [`spec_for`] with the agent-state slot (ADR-0059 §5): for a vendor with
/// hooks — Claude, today — the daemon writes `<store>/sessions/<id>.settings.json`
/// registering the status hook set and passes it as `--settings`, and the
/// PTY environment gets `RALPHY_STATUS_FILE=<store>/sessions/<id>.agent-status.jsonl`.
/// The operator's own `~/.claude/settings.json` keeps its say: the vendor
/// merges `--settings` over it and hook entries are additive. A failed write
/// launches the console WITHOUT the hooks (warned, never refused): a dot is
/// not worth a console.
pub fn spec_with_status(
    agent: Agent,
    root: &Path,
    cwd: PathBuf,
    repo_slug: &str,
    rows: u16,
    cols: u16,
    status: Option<crate::agent_state::StatusFiles>,
) -> SessionSpec {
    let program = match std::env::var_os(AGENT_OVERRIDE_ENV) {
        Some(over) => over,
        None => agent.resolve_program(),
    };
    let mut name = None;
    let mut status_used = None;
    let (args, env) = match agent {
        Agent::Claude => {
            let mut args = Vec::new();
            let mut env = Vec::new();
            if claude_console_named(root) {
                let chosen = console_name(repo_slug);
                args.push(OsString::from("--name"));
                args.push(OsString::from(&chosen));
                name = Some(chosen);
            }
            if let Some(files) = status {
                let exe = PathBuf::from(crate::dispatch::ralphy_exe());
                match files.write(&exe) {
                    Ok(()) => {
                        args.push(OsString::from("--settings"));
                        args.push(files.settings.clone().into_os_string());
                        env.push((
                            OsString::from(crate::agent_state::STATUS_ENV),
                            files.status.clone().into_os_string(),
                        ));
                        status_used = Some(files);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "could not write the console's agent-state hooks; launching without them");
                    }
                }
            }
            (args, env)
        }
        Agent::Gemini => (
            vec![
                OsString::from("--policy"),
                gemini_policy_path(root).into_os_string(),
            ],
            vec![(
                OsString::from("GEMINI_CLI_HOME"),
                gemini_home(root).into_os_string(),
            )],
        ),
        _ => (Vec::new(), Vec::new()),
    };
    SessionSpec {
        program,
        args,
        cwd,
        rows,
        cols,
        env,
        name,
        status: status_used,
    }
}

/// Mint a workbench console's display name: `wb-<repo>-<4 hex>`.
///
/// The `wb-` head is what the operator reads the answer off — a bare
/// `ralphy-3f9c` is the shape a session picks for ITSELF, so without the head a
/// roster row cannot say whether a workbench opened it. `<repo>` is the last
/// segment of the `owner/repo` slug, folded to `[a-z0-9-]` because the name is
/// typed back as an address.
///
/// The suffix is random rather than the daemon's session id: that id is assigned
/// inside [`super::SessionManager::spawn_attached`], AFTER this spec is built, so using
/// it would mean reserving ids in the route. Two CSPRNG bytes, hex, mirroring
/// `auth::generate_token`.
pub fn console_name(repo_slug: &str) -> String {
    let tail = repo_slug.rsplit('/').next().unwrap_or_default();
    let mut repo = String::with_capacity(tail.len());
    for ch in tail.chars() {
        match ch.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9') => repo.push(c),
            // Collapse every run of punctuation into ONE dash, so `my..repo`
            // and `my-repo` do not read as different names.
            _ if !repo.ends_with('-') => repo.push('-'),
            _ => {}
        }
    }
    let repo = repo.trim_matches('-');
    let repo = if repo.is_empty() { "repo" } else { repo };

    let mut bytes = [0u8; 2];
    getrandom::fill(&mut bytes)
        .expect("the OS CSPRNG must be available to name a workbench console");
    format!("wb-{repo}-{:02x}{:02x}", bytes[0], bytes[1])
}

/// Ralphy's owned Gemini configuration root inside a repo: `<repo>/.ralphy/`'s
/// `gemini-home`. This is what `GEMINI_CLI_HOME` names — the CLI appends
/// `.gemini` to it itself (ADR-0043 D4).
///
/// The daemon may not import `ralphy-agent-gemini` (ADR-0032 §10), so the layout
/// is duplicated here; `the_gemini_root_layout_matches_the_adapters_own` reds if
/// the adapter renames either component.
pub fn gemini_home(repo_root: &Path) -> PathBuf {
    repo_root.join(".ralphy").join(GEMINI_ROOT_DIR)
}

/// The policy document inside the owned root:
/// `<repo>/.ralphy/gemini-home/.gemini/ralphy-policy.toml`. Its presence is what
/// the session route gates a Gemini launch on.
pub fn gemini_policy_path(repo_root: &Path) -> PathBuf {
    gemini_home(repo_root)
        .join(GEMINI_CLI_SUBDIR)
        .join(GEMINI_POLICY_FILE)
}

const GEMINI_ROOT_DIR: &str = "gemini-home";

const GEMINI_CLI_SUBDIR: &str = ".gemini";

const GEMINI_POLICY_FILE: &str = "ralphy-policy.toml";

/// Whether the operator opted in to Cursor's codebase upload for `repo_root`,
/// read from `<repo_root>/.ralphy/settings.json`'s
/// `["cursor"]["allow_codebase_indexing_i_understand_the_risk"]`.
///
/// The schema is `ralphy-agent-cursor`'s `CursorSettings`, but the daemon may not
/// import the core (ADR-0032 §10), so it reparses the file — the same precedent
/// `registry.rs` sets for `repos.toml`. `the_optin_key_matches_the_adapters_own_schema`
/// reds if the adapter renames either key.
///
/// INVARIANT: every failure path — no file, unreadable, malformed JSON, wrong
/// type — yields `false`. Refusal is the safe default: an unreadable settings
/// file must never be what opens the upload.
pub fn cursor_indexing_allowed(repo_root: &Path) -> bool {
    let path = repo_root.join(".ralphy").join("settings.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("cursor")?
                .get("allow_codebase_indexing_i_understand_the_risk")?
                .as_bool()
        })
        .unwrap_or(false)
}

/// Whether the operator opted in to Ralphy naming this repo's Claude consoles,
/// read from `<repo_root>/.ralphy/settings.json`'s `["claude"]["console_name"]`.
///
/// The schema is `ralphy-agent-claude`'s `ClaudeSettings`, but the daemon may not
/// import the core (ADR-0032 §10), so it reparses the file — the same precedent
/// `cursor_indexing_allowed` above sets, and `registry.rs` sets for `repos.toml`.
/// `the_console_name_key_matches_the_adapters_own_schema` reds if the adapter
/// renames either the section or the key.
///
/// INVARIANT: every failure path — no file, unreadable, malformed JSON, wrong
/// type — yields `false`. Absent is off: a name is what the operator asks for,
/// and a settings file that cannot be read is not an answer.
pub fn claude_console_named(repo_root: &Path) -> bool {
    let path = repo_root.join(".ralphy").join("settings.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("claude")?.get("console_name")?.as_bool())
        .unwrap_or(false)
}

/// The platform's default interactive shell for the free console (issue #167).
/// Windows prefers `pwsh` (PowerShell 7), falling back to Windows PowerShell,
/// then `%ComSpec%`/`cmd.exe` (both always present, unlike `pwsh`) so the
/// console stays usable on a box without PowerShell 7. Off Windows, the user's
/// login shell (`$SHELL`), falling back to `/bin/sh`.
fn default_shell() -> OsString {
    if cfg!(windows) {
        ralphy_proc_util::locate_program("pwsh")
            .or_else(|| ralphy_proc_util::locate_program("powershell"))
            .map(Into::into)
            .or_else(|| std::env::var_os("ComSpec"))
            .unwrap_or_else(|| "cmd.exe".into())
    } else {
        std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into())
    }
}

/// The free console's working directory: the chosen repo's path, or the home
/// directory when none was chosen, or `.` when even the home directory cannot
/// be resolved (so a console still spawns rather than failing outright).
pub fn console_cwd(repo_path: Option<PathBuf>) -> PathBuf {
    repo_path
        .or_else(ralphy_proc_util::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The argv that makes `shell` run `command` and exit with it: the console IS
/// the command (an `htop` console ends when htop does), so nothing wraps it in
/// a second interactive shell. Dispatched on the shell's file stem, not the
/// platform, because [`default_shell`] falls through several programs on
/// Windows and a test override names none of them:
///
/// - `pwsh`/`powershell` → `-NoLogo -Command <command>`;
/// - `cmd` → `/c <command>`;
/// - anything else → `-lc <command>`. `-l` (login) is deliberate: a monitor
///   tool installed under `~/.local/bin` or by a version manager is on the
///   PATH only once the profile ran, and the operator wrote the command as
///   they would type it at their own prompt.
fn shell_command_args(shell: &Path, command: &str) -> Vec<OsString> {
    // Split on BOTH separators rather than asking `Path` for the stem: on Unix a
    // backslash is not a separator, so `file_stem` of a Windows path is the
    // whole path and PowerShell would be read as a POSIX shell.
    let spelled = shell.to_string_lossy();
    let name = spelled.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = name
        .rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_ascii_lowercase();
    match stem.as_str() {
        "pwsh" | "powershell" => vec!["-NoLogo".into(), "-Command".into(), command.into()],
        "cmd" => vec!["/c".into(), command.into()],
        _ => vec!["-lc".into(), command.into()],
    }
}

/// Build the launch spec for the free console (issue #167): the platform shell
/// in `cwd` — bare when `command` is `None`, else running `command` through
/// [`shell_command_args`] (the startup-command console) — unless
/// `RALPHY_DAEMON_AGENT_OVERRIDE` names a stand-in program instead (the same
/// test seam [`spec_for`] uses; the override still receives the command argv).
pub fn console_spec(cwd: PathBuf, rows: u16, cols: u16, command: Option<&str>) -> SessionSpec {
    let program = match std::env::var_os(AGENT_OVERRIDE_ENV) {
        Some(over) => over,
        None => default_shell(),
    };
    let args = command
        .map(|command| shell_command_args(Path::new(&program), command))
        .unwrap_or_default();
    SessionSpec {
        program,
        args,
        cwd,
        rows,
        cols,
        env: Vec::new(),
        name: None,
        status: None,
    }
}

/// Build a free-console launch through a peer environment's WSL distro.
/// `launcher` is resolved by the caller before the client upgrade; accepting it
/// here keeps argv construction pure and lets tests substitute a portable child.
/// A `command` rides after `--` as `sh -lc <command>`: the distro's login shell
/// is unknown from this side, and `sh -l` still reads the profile that puts
/// `~/.local/bin` on the PATH.
pub fn peer_console_spec(
    launcher: OsString,
    distro: &str,
    peer_path: &Path,
    rows: u16,
    cols: u16,
    command: Option<&str>,
) -> SessionSpec {
    let mut args: Vec<OsString> = vec![
        "-d".into(),
        distro.into(),
        "--cd".into(),
        peer_path.as_os_str().to_owned(),
    ];
    if let Some(command) = command {
        args.extend(["--".into(), "sh".into(), "-lc".into(), command.into()]);
    }
    SessionSpec {
        program: launcher,
        args,
        cwd: console_cwd(None),
        rows,
        cols,
        env: Vec::new(),
        name: None,
        status: None,
    }
}

/// Locate the host-side WSL launcher. The agent override remains the integration
/// test seam, but unlike an agent launch it receives the real WSL argv.
pub fn peer_console_launcher() -> Option<OsString> {
    std::env::var_os(AGENT_OVERRIDE_ENV)
        .or_else(|| ralphy_proc_util::locate_program("wsl.exe").map(Into::into))
}

#[cfg(test)]
mod tests;
