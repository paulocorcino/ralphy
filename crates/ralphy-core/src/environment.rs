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
//! Detection is cheap but not free (one `--version` spawn per found tool), so the
//! runner writes the file once and reuses it: [`ensure_brief`] is a no-op when the
//! file already exists. Everything here is cross-platform — `os_info` for the OS
//! label, [`ralphy_proc_util::find_program`] for PATH resolution (PATHEXT-aware on
//! Windows) — so the same code produces the right brief on Windows, Linux, and
//! macOS.

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
exists because it is common — verify it before a command depends on it. Don't \
install new tools unless the task explicitly asks.
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
/// tools agents reach for in verify/smoke steps. Curated for signal over
/// coverage — niche toolchains (JVM, .NET) are intentionally omitted to keep the
/// brief short; only tools found on this machine appear in the output. `python`
/// and `python3` are both probed because the canonical name differs by OS.
const TOOLS: &[Tool] = &[
    Tool {
        name: "git",
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
];

/// Write `.ralphy/environment.md` once. A no-op when the file already exists so a
/// resumed run reuses the first detection. Best-effort: a detection or write
/// failure just means the charter reads no brief, strictly no worse than today.
pub fn ensure_brief(ws: &Workspace) {
    let path = ws.ralphy_dir().join(ENVIRONMENT_FILE);
    if path.exists() {
        return;
    }
    if let Err(e) = std::fs::write(&path, render(detect())) {
        warn!(error = %e, "writing the environment brief failed");
    }
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
    let os = format!("{} · {}", info, std::env::consts::ARCH);
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

/// Assemble the brief: the fixed header, then one `- ` bullet per fact (OS first,
/// then each detected tool). Kept as a pure function over [`Detected`] so it
/// unit-tests without touching the host.
fn render(d: Detected) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    out.push_str(&format!("- OS: {}\n", d.os));
    for tool in &d.tools {
        out.push_str(&format!("- {tool}\n"));
    }
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
    fn render_lists_os_then_tools_under_the_header() {
        let md = render(Detected {
            os: "Windows 11 · x86_64".into(),
            tools: vec!["git 2.44.0".into(), "node 22.3.0".into()],
        });
        assert!(md.starts_with("# ⚠️ Build environment"));
        assert!(md.contains("never assume a tool exists because it is common"));
        assert!(md.contains("- OS: Windows 11 · x86_64\n"));
        assert!(md.contains("- git 2.44.0\n"));
        assert!(md.contains("- node 22.3.0\n"));
        // The OS bullet precedes the tool bullets.
        assert!(md.find("- OS:").unwrap() < md.find("- git").unwrap());
    }

    #[test]
    fn ensure_brief_writes_once_and_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path());
        std::fs::create_dir_all(ws.ralphy_dir()).unwrap();
        let path = ws.ralphy_dir().join(ENVIRONMENT_FILE);

        ensure_brief(&ws);
        assert!(path.exists(), "brief should be written on first call");

        // A second call must not clobber a hand-edited brief.
        std::fs::write(&path, "sentinel").unwrap();
        ensure_brief(&ws);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sentinel");
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
