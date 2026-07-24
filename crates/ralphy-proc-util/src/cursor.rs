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

/// The opt-out file the vendor honours, and the only one Ralphy will write. Its
/// sibling (the plain ignore file) also stops the upload but DENIES the agent's
/// edit tool, and the agent then routes around the denial through its shell tool
/// — so Ralphy neither writes nor requires it (D6).
const OPT_OUT_FILE: &str = ".cursorindexingignore";

/// The single line the opt-out file must contain to suppress the whole tree.
const OPT_OUT_BODY: &str = "*\n";

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

/// D6's preflight. `Ok(())` when the child may be spawned; `Err` only when the
/// opt-out could not be written (a read-only tree), which is an actionable
/// ADR-0013 stop.
///
/// The upload is a silent default Ralphy will not let happen, but a hard refusal
/// made every Cursor run stop on the operator — so instead Ralphy **creates the
/// opt-out itself and announces it** (`tracing::warn!`), leaving a file the
/// operator can see in `git status` and either commit or delete. This is not a
/// silent write: the notice names the file, the tree it protects, and the opt-in
/// that turns it off.
///
/// Three ways to pass writing nothing: the operator opted in (`allow_indexing`),
/// the cwd is outside any repository, or every enclosing root already carries the
/// opt-out. Otherwise EVERY unprotected enclosing root gets the file — D6 measured
/// a run indexing the parent repository, so an opt-out in the inner root alone
/// would let the outer tree upload.
pub fn indexing_gate(work_dir: &Path, allow_indexing: bool) -> anyhow::Result<()> {
    if allow_indexing {
        return Ok(());
    }
    for root in repo_roots(work_dir)
        .into_iter()
        .filter(|r| !r.join(OPT_OUT_FILE).exists())
    {
        let target = root.join(OPT_OUT_FILE);
        std::fs::write(&target, OPT_OUT_BODY).map_err(|e| {
            anyhow::anyhow!(
                "ralphy: could not write {} to keep this Cursor run from uploading {} to \
                 Cursor's servers: {e}.\nCreate it yourself (one line `*`), or opt in with \
                 `ralphy config set {} true`.",
                target.display(),
                root.display(),
                OPT_IN_KEY,
            )
        })?;
        tracing::warn!(
            "created {} (one line `*`) so this Cursor run does not sync {} to Cursor's servers \
             — it is in your `git status`, review and commit or delete it; opt in with \
             `ralphy config set {} true`",
            target.display(),
            root.display(),
            OPT_IN_KEY,
        );
    }
    Ok(())
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

    /// The contents Ralphy writes: exactly the one line that suppresses the tree.
    fn optout_body(dir: &Path) -> String {
        fs::read_to_string(dir.join(".cursorindexingignore")).expect("opt-out file")
    }

    #[test]
    fn indexing_gate_creates_the_optout_in_a_repo_without_one() {
        let d = repo();
        assert!(
            indexing_gate(d.path(), false).is_ok(),
            "the gate no longer refuses — it writes the opt-out and proceeds"
        );
        // The file it writes is the exact one-line opt-out the vendor honours.
        assert_eq!(optout_body(d.path()), "*\n");
    }

    #[test]
    fn indexing_gate_allows_with_the_optout_file() {
        let d = repo();
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    /// The rule is about the repository ROOT, not the cwd: a run whose working
    /// directory is a nested subdirectory is still uploading the whole repository,
    /// so the opt-out lands at the ROOT even from a deep cwd.
    #[test]
    fn indexing_gate_creates_the_optout_at_the_root_from_a_nested_subdir() {
        let d = repo();
        let nested = d.path().join("crates").join("deep");
        fs::create_dir_all(&nested).unwrap();
        assert!(indexing_gate(&nested, false).is_ok());
        assert_eq!(
            optout_body(d.path()),
            "*\n",
            "the opt-out must be written at the ROOT, not the nested cwd"
        );
        assert!(
            !nested.join(".cursorindexingignore").exists(),
            "nothing is written at the nested cwd — the root covers it"
        );
    }

    /// D6's measured evidence: a run indexed the PARENT repository, not the working
    /// directory it was given. So EVERY enclosing root must get the opt-out — an
    /// inner one alone would leave the outer tree uploading.
    #[test]
    fn indexing_gate_creates_the_optout_in_every_enclosing_repository() {
        let outer = repo();
        let inner = outer.path().join("vendor").join("nested");
        fs::create_dir_all(inner.join(".git")).unwrap();

        // Inner already opted out, outer not: the gate writes the OUTER one and
        // leaves the inner as it found it.
        fs::write(inner.join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(&inner, false).is_ok());
        assert_eq!(
            optout_body(outer.path()),
            "*\n",
            "the outer tree is protected"
        );
        assert_eq!(optout_body(&inner), "*\n", "the inner opt-out is untouched");
    }

    /// D6 explicitly allows this: `draft_issues` / `consolidate_knowledge` may run
    /// where there is no repository, and the gate must not degrade into "refuse
    /// everything", which would make those verbs unreachable.
    #[test]
    fn indexing_gate_allows_when_there_is_no_repository_at_all() {
        let d = tempfile::tempdir().unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    /// The opt-in reaches the capability AND writes nothing: an operator who wants
    /// the indexing must not find an opt-out file suppressing it.
    #[test]
    fn the_opt_in_setting_writes_nothing_and_allows_indexing() {
        let d = repo();
        let before = listing(d.path());
        assert!(
            indexing_gate(d.path(), true).is_ok(),
            "the operator's explicit opt-in must reach the capability"
        );
        assert_eq!(
            listing(d.path()),
            before,
            "opt-in must NOT write the opt-out — that would suppress the indexing the operator asked for"
        );
    }

    /// The gate writes the opt-out exactly once: a run that already carries it, or
    /// one whose operator opted in, leaves the tree byte-for-byte as it found it.
    #[test]
    fn the_gate_writes_nothing_when_already_protected() {
        let d = repo();
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        let before = listing(d.path());
        indexing_gate(d.path(), false).unwrap();
        assert_eq!(
            listing(d.path()),
            before,
            "an already-protected tree must not be rewritten"
        );
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
