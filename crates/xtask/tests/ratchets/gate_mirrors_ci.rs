//! The local gate (`cargo run -p xtask -- gate`) runs the local form of each
//! check step of `ci.yml` and of the test jobs of `release.yml`. A CI step
//! with no gate step, and no reason here why it does not run locally, fails;
//! so does a gate step that names a CI step that is gone.

use super::*;

/// The CI steps with a `run:` that the gate does not run, and why.
const NOT_LOCAL: &[(&str, &str)] = &[
    (
        "Changed files",
        "it lists the files of a pull request, for the fragment step",
    ),
    (
        "A user-visible change carries a fragment",
        "it reads the labels of a pull request; AGENTS.md states the rule",
    ),
    ("Build", "each test step builds what it runs"),
    (
        "Autostart smoke (launchd)",
        "macOS only, and it registers a real launchd agent",
    ),
];

/// `(job, step name)` of each step that has a `run:` key. A step without a
/// name has an empty name, so it cannot match a gate step.
fn run_steps(workflow: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut job = String::new();
    // The indent of the keys of a step, while inside a `steps:` list.
    let mut keys_at: Option<usize> = None;
    let mut current: Option<(String, bool)> = None;
    let mut flush = |current: &mut Option<(String, bool)>, job: &str| {
        if let Some((name, true)) = current.take() {
            found.push((job.to_string(), name));
        }
    };
    for line in workflow.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent <= 2 {
            flush(&mut current, &job);
            keys_at = None;
            job = trimmed.trim_end_matches(':').to_string();
            continue;
        }
        if trimmed == "steps:" {
            keys_at = Some(indent + 4);
            continue;
        }
        let Some(at) = keys_at else { continue };
        let key = match trimmed.strip_prefix("- ") {
            Some(rest) if indent + 2 == at => {
                flush(&mut current, &job);
                current = Some((String::new(), false));
                rest
            }
            _ if indent == at => trimmed,
            _ => continue,
        };
        let Some((name, has_run)) = current.as_mut() else {
            continue;
        };
        if let Some(value) = key.strip_prefix("name:") {
            *name = value.trim().trim_matches('"').to_string();
        } else if key.starts_with("run:") {
            *has_run = true;
        }
    }
    flush(&mut current, &job);
    found
}

/// The names of the CI steps the gate is the local form of.
fn ci_step_names(root: &Path) -> BTreeSet<String> {
    let ci = read(&root.join(".github/workflows/ci.yml"));
    let release = read(&root.join(".github/workflows/release.yml"));
    let release_tests = run_steps(&release)
        .into_iter()
        .filter(|(job, _)| job.ends_with("-test"));
    run_steps(&ci)
        .into_iter()
        .chain(release_tests)
        .map(|(_, name)| name)
        .collect()
}

#[test]
fn the_gate_mirrors_every_ci_step() {
    let ci = ci_step_names(&workspace_root());
    assert!(ci.len() >= 15, "the step scan is blind: {ci:?}");
    let gate: BTreeSet<String> = gate_steps::STEPS
        .iter()
        .flat_map(|s| s.ci.iter().map(ToString::to_string))
        .collect();
    let not_local: BTreeSet<String> = NOT_LOCAL.iter().map(|(n, _)| n.to_string()).collect();
    let unmirrored: Vec<&String> = ci
        .iter()
        .filter(|n| !gate.contains(*n) && !not_local.contains(*n))
        .collect();
    assert!(
        unmirrored.is_empty(),
        "these CI steps have no step in crates/xtask/src/gate/steps.rs, and no reason in \
         NOT_LOCAL: {unmirrored:?}"
    );
    let stale: Vec<&String> = gate
        .union(&not_local)
        .filter(|n| !ci.contains(*n))
        .collect();
    assert!(
        stale.is_empty(),
        "these names are in the gate or in NOT_LOCAL, but no CI step has them: {stale:?}"
    );
}

#[test]
fn the_step_scan_reads_steps_with_a_run_key() {
    let workflow = "\
on:
  push:
    branches: [main]
jobs:
  lint:
    name: a job name is not a step
    steps:
      - uses: actions/checkout@v1
        with:
          name: not a step either
      - name: Format check
        run: cargo fmt --all --check
      - name: Install
        uses: some/action@v1
      - name: \"Block\"
        run: |
          - name: a line of the script
          echo done
  linux-test:
    steps:
      - run: cargo test
      - name: Doctests
        env:
          A: b
        run: cargo test --doc
";
    let found = run_steps(workflow);
    let pairs: Vec<(&str, &str)> = found
        .iter()
        .map(|(j, n)| (j.as_str(), n.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("lint", "Format check"),
            ("lint", "Block"),
            ("linux-test", ""),
            ("linux-test", "Doctests"),
        ]
    );
}
