use super::*;

/// The WIRING half of D6, and the invariant the whole slice exists for: no test
/// here spawns a real child, so deleting the gate call would keep the suite
/// green and let a run spawn before the opt-out is written. Pin the call AND its
/// position — a gate that runs after the spawn has already uploaded the
/// repository. Fragments are assembled with `concat!` so the assertion cannot
/// match itself.
#[test]
fn the_gate_runs_before_any_child_is_spawned() {
    let src = include_str!("../../outcome.rs");
    let gate = concat!("indexing_gate(", "work_dir, self.allow_indexing)?;");
    let seed = concat!("seed_cursor_config", "_dir(");
    let spawn = concat!("HeadlessCall::", "new(cmd,");
    let at_gate = src
        .find(gate)
        .expect("run_cursor must call the indexing gate");
    let at_seed = src.find(seed).expect("run_cursor must seed the config dir");
    let at_spawn = src.find(spawn).expect("the HeadlessCall site moved");
    assert!(
        at_gate < at_spawn,
        "D6 must write the opt-out BEFORE the child is spawned, not after"
    );
    assert!(
        at_seed < at_spawn,
        "D17's isolation must be seeded BEFORE the child is spawned"
    );
    assert_eq!(
        src.matches(spawn).count(),
        1,
        "this is the crate's single HeadlessCall site (ADR-0040 Tier 1)"
    );
}

/// The pin above counts spawns in ONE file, which is structurally blind to a
/// child spawned from another module — and one exists (`auth::probe_cursor_login`).
/// So enumerate every spawn site in the crate and assert each one is accounted
/// for: either it is gated (the run path) or it runs in a throwaway cwd and
/// config dir, where D6 has nothing to refuse and D17 nothing to protect.
/// Recursive, so an ADR-0022 `foo.rs` + `foo/` split cannot silently drop a file.
#[test]
fn every_spawn_site_in_the_crate_is_gated_or_neutralized() {
    fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let path = entry.expect("entry").path();
            if path.file_stem().and_then(|s| s.to_str()) == Some("tests") {
                // A sibling test module or its `tests/` folder (ADR-0022 §3)
                // carries no `#[cfg(test)]` marker of its own; it is test code.
            } else if path.is_dir() {
                sources(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let body = std::fs::read_to_string(&path).expect("read source");
                let production = crate::tests::production_text(&body).to_string();
                out.push((path.display().to_string(), production));
            }
        }
    }
    let mut files = Vec::new();
    sources(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut files,
    );

    let ctor = concat!("Command::", "new(");
    let spawners = [
        concat!("HeadlessCall::", "new("),
        concat!("run_", "headless("),
        // The one-shots spawn through the shared harness, not `HeadlessCall`
        // directly — without these the whole of `tasks.rs` is invisible here.
        // `run_json_session` is the harness' third public entrypoint (the one
        // the Claude adapter uses); it is listed so a future file reaching for
        // it cannot slip past this scan.
        concat!("run_init_", "session("),
        concat!("run_text_", "session("),
        concat!("run_json_", "session("),
    ];
    let mut sites: Vec<String> = Vec::new();
    for (name, body) in &files {
        if body.contains(ctor) || spawners.iter().any(|s| body.contains(s)) {
            sites.push(name.clone());
        }
    }
    sites.sort();
    let short: Vec<&str> = sites
        .iter()
        .map(|s| s.rsplit(['\\', '/']).next().unwrap_or(s))
        .collect();
    assert_eq!(
        short,
        vec!["auth.rs", "command.rs", "outcome.rs", "tasks.rs"],
        "a NEW child-spawning file appeared — decide its D6/D17/D18 stance and \
             extend this test; a spawn that skips them is the failure this slice exists to prevent"
    );

    // D6/D17 must cover EVERY one-shot spawn — stated as a RELATION, not a
    // literal count: a legitimate fifth verb must not red this, and a preflight
    // moved BELOW its spawn must. Pairs each spawn with the preflight that
    // precedes it, the same positional shape as the run path's pin above.
    let tasks = &files
        .iter()
        .find(|(n, _)| n.ends_with("tasks.rs"))
        .expect("tasks.rs")
        .1;
    let preflight = concat!("one_shot_", "preflight(");
    // Skip the fn's own definition; what remains are the call sites.
    let calls: Vec<usize> = tasks
        .match_indices(preflight)
        .map(|(i, _)| i)
        .skip(1)
        .collect();
    let mut spawns: Vec<usize> = spawners
        .iter()
        .flat_map(|s| tasks.match_indices(s).map(|(i, _)| i))
        .collect();
    spawns.sort_unstable();
    assert!(!spawns.is_empty(), "tasks.rs must hold the one-shot spawns");
    assert_eq!(
        calls.len(),
        spawns.len(),
        "every one-shot spawn needs its own gate+seed call"
    );
    for (call, spawn) in calls.iter().zip(&spawns) {
        assert!(
            call < spawn,
            "a one-shot spawns at byte {spawn} before its preflight at {call}"
        );
    }

    // `command.rs` only BUILDS the command; the run path's gate is pinned above.
    let auth = &files
        .iter()
        .find(|(n, _)| n.ends_with("auth.rs"))
        .expect("auth.rs")
        .1;
    assert!(
        auth.contains(concat!("current_", "dir(scratch.path())")),
        "the login probe must run OUTSIDE the operator's repository (D6)"
    );
    for key in ["CURSOR_CONFIG_DIR", "CURSOR_AGENT_DISABLE_DEBUG_LOG"] {
        assert!(
            auth.contains(key),
            "the login probe must set {key} like every other invocation (D17/D18)"
        );
    }
}

/// `shellToolCall.result.success` carries no file-change data at all, so a
/// progress number read from the stream would report zero for shell work. The
/// production half must never name those fields.
#[test]
fn no_progress_read_from_the_stream() {
    let production = crate::tests::production_text(include_str!("../../outcome.rs"));
    for banned in ["linesAdded", "linesRemoved", "diffString"] {
        assert!(
            !production.contains(banned),
            "progress comes from the HEAD-diff, never the stream; found {banned}"
        );
    }
}

/// Every committed fixture must be read by a test via `include_str!`, not
/// re-inlined as a literal record — the fixture rule this whole slice exists
/// to enforce. Recursive over `src/` so a future ADR-0022 split cannot hide a
/// fixture from the check.
#[test]
fn every_fixture_is_read_by_a_test() {
    fn sources(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(std::fs::read_to_string(&path).expect("read source"));
            }
        }
    }
    let mut sources_text = Vec::new();
    sources(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut sources_text,
    );

    let fixtures_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"));
    for entry in std::fs::read_dir(fixtures_dir).expect("readable fixtures dir") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("fixture file name");
        // Read from `src/` or from a sibling test module one level down, so the
        // needle is the file name and the macro, not the depth.
        let tail = format!("fixtures/{name}\")");
        let read = sources_text.iter().any(|src| {
            src.lines()
                .any(|l| l.contains(concat!("include_str!(", "\"")) && l.contains(&tail))
        });
        assert!(
            read,
            "fixture {name} is committed but no test reads it via include_str!"
        );
    }

    let result_literal = concat!("\"type\"", ": \"result\"");
    // The tests are this file (ADR-0022 §3), so the count is over it directly.
    let test_half = include_str!("../tests.rs");
    assert!(
        test_half.matches(result_literal).count() <= 2,
        "a new record shape must be captured as a fixture, not inlined — \
             the two never-reproduced synthetic tests are the only exception"
    );
}
