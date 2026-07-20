//! The build-environment brief (`.ralphy/environment.md`): a short, OS-agnostic
//! note that orients any agent — planner or executor — to the machine its
//! commands actually run on. It exists to stop the classic autonomous-agent
//! failure of writing POSIX-only verify steps (a `netstat`-based smoke script,
//! a bare `python3`) on a host that doesn't have them: the plan bounces the
//! verify gate forever because the failure is environmental, not a code bug the
//! agent can fix.
//!
//! The brief is a *lead*, deliberately not exhaustive of the whole machine — it
//! reports the OS and the small set of common toolchains the runner probes, each
//! with its detected version. The prose tells the agent to verify anything not
//! listed before depending on it, so omission never becomes a false "installed".
//!
//! The brief is regenerated at the start of every run, not cached: it now carries
//! one run-specific fact — the live PID of the orchestrator process that must never
//! be killed — so a stale copy from an earlier run would name a dead (or recycled)
//! PID. Re-probing the toolchain once per run start (a `--version` spawn per found
//! tool) is negligible against a run's lifetime. Everything here is cross-platform
//! — `os_info` for the OS label, [`ralphy_proc_util::find_program`] for PATH
//! resolution (PATHEXT-aware on Windows), and `std::env::current_exe` /
//! `std::process::id` for the orchestrator identity — so the same code produces the
//! right brief on Windows, Linux, and macOS.

use std::process::Command;

use regex::Regex;
use tracing::warn;

use crate::Workspace;

/// The scratch file the runner drops in the workspace so any adapter's charter
/// can read the machine it builds on. Vendor-neutral, mirroring the
/// `verify-failure.md` / `handoffs.md` channel.
const ENVIRONMENT_FILE: &str = "environment.md";

/// The attention header + usage rule, prepended to the detected facts. OS-neutral
/// on purpose: it holds true on Windows, Linux, and macOS, so a single template
/// serves every host (no per-OS prose to drift).
const HEADER: &str = "\
# ⚠️ Build environment — read before writing any shell command

You are working on the machine below. Adapt every command — build steps, \
`## Verify`, smoke scripts — to this OS and the project's needs. These are the \
tools confirmed present; the machine may have others, but never assume a tool \
exists because it is common — verify it before a command depends on it. \
Equally, this list is a lead, not an inventory: a tool missing from it may \
still be installed, so never cite its absence here as proof it is unavailable \
— the only valid evidence of absence is a probe you ran that failed (e.g. \
`tool --version`). Don't install new tools unless the task explicitly asks — \
with one standing exception: a headless-browser driver (e.g. Playwright) \
needed to verify a browser-facing acceptance criterion.
";

/// One probed toolchain: the label shown to the agent and the argument that makes
/// it print its version. Most CLIs use `--version`; Go is the notable exception
/// (`go version`). Version output is normalized to a bare `x.y.z` token, so the
/// exact wording of each tool's banner does not matter.
struct Tool {
    name: &'static str,
    version_arg: &'static str,
}

/// The probe list: common language runtimes, package managers, and the shell
/// tools agents reach for in verify/smoke steps (`gh` included — Ralphy's own
/// workflow is GitHub-driven, and its omission once read as "not installed"). Curated for signal over
/// coverage — niche toolchains (JVM, .NET) are intentionally omitted to keep the
/// brief short; only tools found on this machine appear in the output. `python`
/// and `python3` are both probed because the canonical name differs by OS.
const TOOLS: &[Tool] = &[
    Tool {
        name: "git",
        version_arg: "--version",
    },
    Tool {
        name: "gh",
        version_arg: "--version",
    },
    Tool {
        name: "node",
        version_arg: "--version",
    },
    Tool {
        name: "npm",
        version_arg: "--version",
    },
    Tool {
        name: "python",
        version_arg: "--version",
    },
    Tool {
        name: "python3",
        version_arg: "--version",
    },
    Tool {
        name: "pip",
        version_arg: "--version",
    },
    Tool {
        name: "cargo",
        version_arg: "--version",
    },
    Tool {
        name: "go",
        version_arg: "version",
    },
    Tool {
        name: "ruby",
        version_arg: "--version",
    },
    Tool {
        name: "bundler",
        version_arg: "--version",
    },
    Tool {
        name: "php",
        version_arg: "--version",
    },
    Tool {
        name: "composer",
        version_arg: "--version",
    },
    Tool {
        name: "docker",
        version_arg: "--version",
    },
    Tool {
        name: "curl",
        version_arg: "--version",
    },
    Tool {
        name: "make",
        version_arg: "--version",
    },
    Tool {
        name: "playwright",
        version_arg: "--version",
    },
];

/// Write `.ralphy/environment.md`, refreshing it on every run. Unlike the machine
/// facts (static), the orchestrator-guard section carries this run's live PID, so a
/// stale file from an earlier run would name a dead or recycled process — hence no
/// write-once short-circuit. Best-effort: a detection or write failure just means
/// the charter reads no brief, strictly no worse than today.
pub fn ensure_brief(ws: &Workspace) {
    let path = ws.ralphy_dir().join(ENVIRONMENT_FILE);
    if let Err(e) = std::fs::write(&path, render(detect(), &orchestrator_guard())) {
        warn!(error = %e, "writing the environment brief failed");
    }
}

/// The live orchestrator identity for [`guard_section`]: this process's executable
/// path and PID, read fresh each run. `current_exe` is stable (same binary) but the
/// PID is not — which is exactly why the brief cannot be cached.
fn orchestrator_guard() -> String {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "the ralphy executable".to_string());
    guard_section(&exe, std::process::id())
}

/// The DO-NOT-KILL notice, pure over its inputs so it unit-tests without touching
/// the host. It names the executable and PID, but leads with the NAME-match hazard:
/// the workbench daemon is the same `ralphy` binary, so a by-name kill (the actual
/// failure this section exists to prevent) takes the orchestrator down with it.
fn guard_section(exe: &str, pid: u32) -> String {
    format!(
        "## ⛔ DO NOT KILL the orchestrator\n\
        \n\
        `{exe}` is Ralphy — the agent orchestrator that launched this session and is \
        monitoring it. It is the parent of your own process; killing it ends the \
        whole run instantly.\n\
        \n\
        - DO NOT KILL any process by the NAME `ralphy` / `ralphy.exe`. The workbench \
        daemon is the SAME binary, so `Stop-Process ralphy`, `taskkill /IM \
        ralphy.exe`, `pkill ralphy`, and `killall ralphy` all hit the orchestrator \
        too. To stop a daemon you started, target its exact PID or port — never a \
        name match.\n\
        - Live orchestrator PID on this host: {pid} — never signal, kill, or \
        force-stop it.\n"
    )
}

/// The OS label and every probed tool that resolved, gathered from the live host.
struct Detected {
    os: String,
    tools: Vec<String>,
}

/// Probe the host: the OS name+version+arch, then each tool in [`TOOLS`] that
/// resolves on `PATH`, formatted as `name x.y.z` (or bare `name` when the version
/// can't be parsed).
fn detect() -> Detected {
    let info = os_info::get();
    let os = format!(
        "{} · {}{}",
        info,
        std::env::consts::ARCH,
        wsl_suffix().map(|w| format!(" ({w})")).unwrap_or_default(),
    );
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    let tools = TOOLS
        .iter()
        .filter_map(|t| probe(t, path.clone(), pathext.clone()))
        .collect();
    Detected { os, tools }
}

/// Resolve one tool on `PATH` and, if found, run its version command. Returns
/// `name x.y.z` when a version parses, `name` when the tool runs but its version
/// is unrecognizable, and `None` when the tool is absent or won't launch.
fn probe(
    tool: &Tool,
    path: Option<std::ffi::OsString>,
    pathext: Option<std::ffi::OsString>,
) -> Option<String> {
    let exe = ralphy_proc_util::find_program(tool.name, path, pathext)?;
    let out = Command::new(&exe).arg(tool.version_arg).output().ok()?;
    // Some tools print their version to stderr (and old pythons did too), so read
    // both streams before giving up.
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr)
    } else {
        String::from_utf8_lossy(&out.stdout)
    };
    Some(match parse_version(&text) {
        Some(v) => format!("{} {v}", tool.name),
        None => tool.name.to_string(),
    })
}

/// Pull the first `x.y[.z…]` token out of a version banner: `git version 2.44.0`
/// → `2.44.0`, `v22.3.0` → `22.3.0`. Returns `None` when no dotted-number run is
/// present, so the caller falls back to the bare tool name.
fn parse_version(text: &str) -> Option<String> {
    // A run of at least two dot-separated number groups — enough to skip stray
    // single integers in a banner while matching real versions.
    let re = Regex::new(r"\d+(?:\.\d+)+").ok()?;
    re.find(text).map(|m| m.as_str().to_string())
}

/// The WSL flavor of the host, or `None` when not under WSL (native Linux,
/// Windows, macOS). Read from `/proc/version` — absent off Linux, so the read
/// simply fails and yields `None` without any platform gate. os_info reports the
/// underlying distro under WSL and never surfaces this, yet it is the exact
/// signal that separates a real Linux box from one where a Windows toolchain may
/// be reachable (or, on WSL1, unreachable) — the class of trap this brief exists
/// to flag.
fn wsl_suffix() -> Option<&'static str> {
    let text = std::fs::read_to_string("/proc/version").ok()?;
    classify_wsl(&text)
}

/// Classify a `/proc/version` string: WSL2 kernels carry `WSL2`, WSL1 carries
/// `microsoft` without it. Pure over its input so it unit-tests without a Linux
/// host.
fn classify_wsl(proc_version: &str) -> Option<&'static str> {
    let lower = proc_version.to_ascii_lowercase();
    if lower.contains("wsl2") {
        Some("WSL2")
    } else if lower.contains("microsoft") {
        Some("WSL1")
    } else {
        None
    }
}

/// Assemble the brief: the fixed header, one `- ` bullet per fact (OS first, then
/// each detected tool), then the orchestrator-guard notice last so the machine
/// facts stay contiguous under the header. Pure over its inputs ([`Detected`] plus
/// the pre-rendered `guard`) so it unit-tests without touching the host.
fn render(d: Detected, guard: &str) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    out.push_str(&format!("- OS: {}\n", d.os));
    for tool in &d.tools {
        out.push_str(&format!("- {tool}\n"));
    }
    out.push('\n');
    out.push_str(guard);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_pulls_first_dotted_token() {
        assert_eq!(
            parse_version("git version 2.44.0").as_deref(),
            Some("2.44.0")
        );
        assert_eq!(parse_version("v22.3.0").as_deref(), Some("22.3.0"));
        assert_eq!(
            parse_version("Docker version 27.1.1, build abc").as_deref(),
            Some("27.1.1")
        );
        assert_eq!(
            parse_version("go version go1.22.4 linux/amd64").as_deref(),
            Some("1.22.4")
        );
    }

    #[test]
    fn parse_version_none_without_a_dotted_number() {
        assert_eq!(parse_version("no version here"), None);
        // A lone integer is not a version — needs at least two groups.
        assert_eq!(parse_version("build 12345"), None);
    }

    #[test]
    fn classify_wsl_separates_wsl2_wsl1_and_native() {
        // WSL2 kernel banner.
        assert_eq!(
            classify_wsl("Linux version 5.15.90.1-microsoft-standard-WSL2 (oe-user@oe-host) ..."),
            Some("WSL2")
        );
        // WSL1 banner: carries Microsoft but not WSL2.
        assert_eq!(
            classify_wsl("Linux version 4.4.0-19041-Microsoft (Microsoft@Microsoft.com) ..."),
            Some("WSL1")
        );
        // Native Linux — no WSL marker.
        assert_eq!(
            classify_wsl("Linux version 6.5.0-14-generic (buildd@lcy02) ..."),
            None
        );
    }

    #[test]
    fn render_lists_os_then_tools_then_guard_under_the_header() {
        let md = render(
            Detected {
                os: "Windows 11 · x86_64".into(),
                tools: vec!["git 2.44.0".into(), "node 22.3.0".into()],
            },
            "## ⛔ DO NOT KILL the orchestrator\n",
        );
        assert!(md.starts_with("# ⚠️ Build environment"));
        assert!(md.contains("never assume a tool exists because it is common"));
        // The inverse trap — treating omission from the list as proof of
        // absence — must be closed explicitly (an executor once left a
        // criterion review-only citing "no gh per environment.md" on a host
        // that had gh).
        assert!(md.contains("the only valid evidence of absence is a probe you ran"));
        assert!(md.contains("## ⛔ DO NOT KILL the orchestrator"));
        assert!(md.contains("- OS: Windows 11 · x86_64\n"));
        assert!(md.contains("- git 2.44.0\n"));
        assert!(md.contains("- node 22.3.0\n"));
        // Machine facts stay contiguous under the header (OS then tools); the guard
        // lands last, below the OS/tool bullets.
        assert!(md.find("- OS:").unwrap() < md.find("- git").unwrap());
        assert!(md.find("- git").unwrap() < md.find("DO NOT KILL").unwrap());
    }

    #[test]
    fn guard_section_leads_with_name_hazard_and_names_pid() {
        let s = guard_section("C:\\ralphy\\ralphy.exe", 54636);
        assert!(s.contains("## ⛔ DO NOT KILL the orchestrator"));
        assert!(s.contains("C:\\ralphy\\ralphy.exe"));
        assert!(s.contains("54636"));
        // The by-NAME prohibition — the actual failure this section prevents — must
        // be explicit, naming the cross-platform kill idioms.
        assert!(s.contains("NAME"));
        assert!(s.contains("Stop-Process ralphy"));
        assert!(s.contains("pkill ralphy"));
    }

    #[test]
    fn ensure_brief_refreshes_each_run_and_names_the_orchestrator() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path());
        std::fs::create_dir_all(ws.ralphy_dir()).unwrap();
        let path = ws.ralphy_dir().join(ENVIRONMENT_FILE);

        ensure_brief(&ws);
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("DO NOT KILL the orchestrator"));
        // This run's live PID is named so a by-PID kill can avoid it.
        assert!(first.contains(&std::process::id().to_string()));
        // The tools brief still rides along under the guard.
        assert!(first.contains("never assume a tool exists because it is common"));

        // A stale copy MUST be overwritten — the PID it carries is run-specific, so
        // (unlike the old write-once brief) reuse would name the wrong process.
        std::fs::write(&path, "stale").unwrap();
        ensure_brief(&ws);
        let second = std::fs::read_to_string(&path).unwrap();
        assert_ne!(second, "stale");
        assert!(second.contains("DO NOT KILL the orchestrator"));
    }

    #[test]
    fn detect_finds_at_least_the_host_git() {
        // The repo is a git checkout and CI has git on PATH, so detection must
        // surface it — a live, cross-platform smoke test of the probe path.
        let d = detect();
        assert!(!d.os.is_empty());
        assert!(
            d.tools.iter().any(|t| t.starts_with("git")),
            "expected git among {:?}",
            d.tools
        );
    }
}
