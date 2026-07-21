//! Cursor-specific path knowledge and the one policy gate that must hold wherever
//! `cursor-agent` is spawned from.
//!
//! It lives here, below both `ralphy-core` and the daemon, because two independent
//! spawn paths need it: the run path (`ralphy-agent-cursor`) and the workbench's
//! interactive PTY launch. ADR-0032 §10 forbids the daemon importing the core, so
//! the adapter crate — which does import it — cannot be that shared home. One
//! implementation beats two that can disagree about a product-stance refusal
//! (ADR-0042 D19).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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
pub fn locate_cursor_with(
    path_var: Option<OsString>,
    pathext: Option<OsString>,
    home: Option<PathBuf>,
    localappdata: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(found) =
        crate::locate_program_with(NAMES[0], path_var.clone(), pathext.clone(), home.clone())
    {
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
    crate::locate_program_with(NAMES[1], path_var, pathext, home)
}

/// Locate the Cursor CLI against the real environment. `None` means the vendor is
/// not installed — `ralphy init`'s gate reports presence through this, never
/// through `locate_program("cursor")`, which would look for the wrong binary name.
pub fn locate_cursor() -> Option<PathBuf> {
    locate_cursor_with(
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
        crate::home_dir(),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    )
}

/// The opt-out file the vendor honours, and the only one Ralphy will accept. Its
/// sibling (the plain ignore file) also stops the upload but DENIES the agent's
/// edit tool, and the agent then routes around the denial through its shell tool
/// — so Ralphy neither writes nor requires it (D6).
const OPT_OUT_FILE: &str = ".cursorindexingignore";

/// The persisted key that overrides the refusal, quoted verbatim in the message
/// so the operator can copy it into `ralphy config set`.
const OPT_IN_KEY: &str = "cursor.allow_codebase_indexing_i_understand_the_risk";

/// Every enclosing repository root, outermost LAST. Empty when the path is not
/// inside a repository at all — `draft_issues` and `consolidate_knowledge` may
/// legitimately run there, and D6 lets them through: there is nothing to upload
/// and nowhere to put the file.
///
/// The walk does NOT stop at the first `.git`. D6 records, as measured evidence,
/// that a run indexed the **parent repository** rather than the working directory
/// it was given — so for a repo checked out inside another (or a submodule), an
/// opt-out in the inner root alone would let the outer tree upload silently. Every
/// root found must carry the file.
fn repo_roots(start: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            roots.push(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    roots
}

/// D6's preflight. `Ok(())` when the child may be spawned; `Err` with an
/// actionable ADR-0013 stop otherwise.
///
/// Three ways to pass: the operator opted in (`allow_indexing`), the cwd is
/// outside any repository, or the repository root carries the opt-out file.
pub fn indexing_gate(work_dir: &Path, allow_indexing: bool) -> anyhow::Result<()> {
    if allow_indexing {
        return Ok(());
    }
    // The OUTERMOST unprotected root is the one worth naming: it is the largest
    // tree that would be uploaded, and protecting it is what the operator must do.
    let Some(root) = repo_roots(work_dir)
        .into_iter()
        .rfind(|r| !r.join(OPT_OUT_FILE).exists())
    else {
        return Ok(());
    };
    anyhow::bail!(
        "ralphy: refusing to run `cursor` in {} — an ordinary Cursor run walks this \
         repository and syncs a copy of it to Cursor's servers, whatever the task asked for.\n\
         Opt out by creating {}/{} containing one line:\n\
         \n    *\n\n\
         Ralphy will not create that file for you: it lands in your repository and your \
         `git status`, so it is your call.\n\
         If you WANT the indexing, opt in instead:\n\
         \n    ralphy config set {} true\n",
        root.display(),
        root.display(),
        OPT_OUT_FILE,
        OPT_IN_KEY,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A temp directory that LOOKS like a git repository to the walk above.
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        fs::create_dir(d.path().join(".git")).expect("mkdir .git");
        d
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// D14: the vendor is on `PATH` on neither platform, under either of its two
    /// names. Each known install shape must resolve with an EMPTY `PATH`.
    #[test]
    fn locate_cursor_finds_each_install_shape() {
        // A file the platform would actually run: on Unix `locate_program_with`
        // requires an execute bit, and on Windows a bare name needs `PATHEXT`.
        fn touch_exe(p: &Path) {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
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
    fn indexing_gate_refuses_a_repo_without_the_optout() {
        let d = repo();
        let err = indexing_gate(d.path(), false)
            .expect_err("a repository with no opt-out must refuse the spawn");
        let msg = err.to_string();
        // The message must be actionable, not merely a refusal: it names the file,
        // its one-line content, and the key that overrides it.
        assert!(msg.contains(".cursorindexingignore"), "{msg}");
        assert!(msg.contains('*'), "{msg}");
        assert!(
            msg.contains("cursor.allow_codebase_indexing_i_understand_the_risk"),
            "{msg}"
        );
    }

    #[test]
    fn indexing_gate_allows_with_the_optout_file() {
        let d = repo();
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    /// The rule is about the repository ROOT, not the cwd: a run whose working
    /// directory is a nested subdirectory is still uploading the whole repository.
    #[test]
    fn indexing_gate_resolves_the_root_from_a_nested_subdir() {
        let d = repo();
        let nested = d.path().join("crates").join("deep");
        fs::create_dir_all(&nested).unwrap();
        assert!(
            indexing_gate(&nested, false).is_err(),
            "a nested cwd must resolve the enclosing root"
        );
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(
            indexing_gate(&nested, false).is_ok(),
            "the opt-out at the ROOT covers a nested cwd"
        );
    }

    /// D6's measured evidence: a run indexed the PARENT repository, not the working
    /// directory it was given. So an opt-out in an inner repository alone must not
    /// pass — the outer tree is what would be uploaded.
    #[test]
    fn indexing_gate_requires_the_optout_in_every_enclosing_repository() {
        let outer = repo();
        let inner = outer.path().join("vendor").join("nested");
        fs::create_dir_all(inner.join(".git")).unwrap();

        // Inner opted out, outer not: still refused, and the message names the OUTER
        // root — the larger tree, and the one the operator has to protect.
        fs::write(inner.join(".cursorindexingignore"), "*\n").unwrap();
        let err = indexing_gate(&inner, false)
            .expect_err("an inner opt-out must not cover the enclosing repository");
        assert!(
            err.to_string()
                .contains(&outer.path().display().to_string()),
            "{err}"
        );

        // Both opted out: allowed.
        fs::write(outer.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(&inner, false).is_ok());
    }

    /// D6 explicitly allows this: `draft_issues` / `consolidate_knowledge` may run
    /// where there is no repository, and the gate must not degrade into "refuse
    /// everything", which would make those verbs unreachable.
    #[test]
    fn indexing_gate_allows_when_there_is_no_repository_at_all() {
        let d = tempfile::tempdir().unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    #[test]
    fn the_opt_in_setting_overrides_the_refusal() {
        let d = repo();
        assert!(indexing_gate(d.path(), false).is_err());
        assert!(
            indexing_gate(d.path(), true).is_ok(),
            "the operator's explicit opt-in must reach the capability"
        );
    }

    /// D6: Ralphy never creates the opt-out file, and the gate is a pure read on
    /// BOTH paths — the refusing one and the allowing one.
    #[test]
    fn the_gate_writes_nothing() {
        let d = repo();
        let before = listing(d.path());
        let _ = indexing_gate(d.path(), false);
        assert_eq!(listing(d.path()), before, "the refusal must write nothing");

        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        let before = listing(d.path());
        indexing_gate(d.path(), false).unwrap();
        assert_eq!(listing(d.path()), before, "the pass must write nothing too");
    }

    /// D6: the sibling ignore file denies the vendor's edit tool, so Ralphy must
    /// never write it, require it, or even name it. The gate's home moved here, so
    /// the scan follows it — the adapter crate keeps its own copy over its `src/`.
    #[test]
    fn no_cursorignore_in_proc_util() {
        fn scan(dir: &Path, needle: &str, hits: &mut Vec<String>) {
            for entry in fs::read_dir(dir).expect("src/ is readable") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    scan(&path, needle, hits);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                if fs::read_to_string(&path)
                    .expect("read source")
                    .contains(needle)
                {
                    hits.push(path.display().to_string());
                }
            }
        }
        let needle = concat!(".cursor", "ignore");
        let mut hits = Vec::new();
        scan(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            needle,
            &mut hits,
        );
        assert!(
            hits.is_empty(),
            "the plain ignore file breaks the vendor's edit tool (D6); found in {hits:?}"
        );
    }
}
