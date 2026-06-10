# Plan for #25: OpenCode adapter — skills provisioned under `.ralphy/skills` via injected `skills.paths`

## Feasible: yes
The work is a retargeted near-clone of the existing `materialize_codex_skills`
(`crates/ralphy-agent-codex/src/lib.rs:36`) plus an env injection on the child
`Command`, both with direct prior art in the Codex adapter and ADR-0005 D7. All
touch points are concrete and unit-test-verifiable.

## Execution model: sonnet
Mechanical, localized work mirroring an existing function: embed an asset tree,
clear-and-re-extract it, write a `.gitignore`, build a small JSON config string,
and inject it as an env var — all confined to one crate with an established
template. No tricky design or cross-cutting concerns.

## Done when
- `cargo test -p ralphy-agent-opencode` passes, including a new test that
  asserts `materialize_opencode_skills` extracts `reviewer/SKILL.md`,
  `staged-plan/SKILL.md`, and the multi-file `reviewer/scripts/audit.py` under
  `<repo>/.ralphy/skills`, writes `<repo>/.ralphy/.gitignore`, and is idempotent
  across a second call.
- `cargo test -p ralphy-agent-opencode` passes, including a new test that parses
  the injected `OPENCODE_CONFIG_CONTENT` string back as JSON and asserts
  `skills.paths` contains the materialized `.ralphy/skills` directory (proving
  the content is well-formed and the path matches the materialized layout).
- `cargo build` succeeds and `cargo fmt --check` / `cargo clippy` produce no new
  warnings.
- Review-only: a live `opencode run` discovers and can invoke the `reviewer`
  and `staged-plan` skills from `.ralphy/skills/`, and no file is written
  outside `.ralphy/`.

## Acceptance ledger
- [review-only] A run shows OpenCode discovering and able to invoke the `reviewer` skill (and `staged-plan`) from `.ralphy/skills/`. — evidence: a human runs `opencode run` against this branch and confirms the reviewer/staged-plan skills are invocable; the unit test proves the files materialize and the path is injected, but only a live run proves OpenCode discovers them.
- [review-only] Nothing is written outside `.ralphy/`; the materialized tree is git-ignored and never appears in an executor commit. — evidence: the unit test asserts the tree lands under `.ralphy/skills` and `.ralphy/.gitignore` (`*`) is written; a human confirms in the PR that no executor commit contains `.ralphy/skills` files.
- [verified] The deferred `skills.paths` granularity is resolved: confirm whether each entry is the container dir (`.ralphy/skills`) or a per-skill dir, and the materialized layout + injected path match. — evidence: resolved under `## Decisions` (container dir); the config test asserts the single injected path equals the materialized container `.ralphy/skills`.
- [verified] A unit test asserts the skills (incl. multi-file `reviewer/scripts/*`) materialize and the injected config content is well-formed, mirroring `materialize_codex_skills`'s test. — evidence: the two new tests above (materialize + config) cover exactly this.

## Decisions
- Decision: each `skills.paths` entry is the single **container** dir
  (`<abs>/.ralphy/skills`), not a per-skill dir. Why: it is the natural reading
  of OpenCode's "Additional paths to skill folders" schema key and mirrors how
  `materialize_codex_skills` lays skills out as subdirs under one container;
  per-skill entries would need enumeration logic for no benefit (ADR-0005 D7).
- Decision: write `<repo>/.ralphy/.gitignore` with `*` (mirroring Codex's
  `.agents/.gitignore`). Why: makes the materialized tree self-contained-ignored
  even though the core already adds `.ralphy/` to the target repo's root
  `.gitignore`, exactly as the issue asks.

## Steps
- [x] In `crates/ralphy-agent-opencode/Cargo.toml`, add `include_dir.workspace = true`
      to `[dependencies]` (Codex's Cargo.toml already has it).
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, add
      `use include_dir::{include_dir, Dir};` and a
      `static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");`
      embedding the skills tree at build time (clone of the Codex `SKILLS`).
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, add
      `fn materialize_opencode_skills(ws: &Workspace) -> Result<PathBuf>` that
      clears `<repo>/.ralphy/skills`, re-extracts `SKILLS` into it, writes
      `<repo>/.ralphy/.gitignore` (`*`), and returns the `.ralphy/skills` path —
      a retarget of `materialize_codex_skills`.
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, add
      `fn opencode_skills_config(skills_dir: &Path) -> String` that builds
      `{"skills":{"paths":["<abs skills_dir>"]}}` via `serde_json::json!`/
      `to_string` (canonicalizing `skills_dir`, falling back to the path as-is),
      returning the string injected as `OPENCODE_CONFIG_CONTENT`.
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, extend
      `build_opencode_command` with a `skills_config: &str` parameter and add
      `.env("OPENCODE_CONFIG_CONTENT", skills_config)`, so the injection is the
      single shared invocation point; update its existing unit-test call sites
      to pass a value.
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, in `OpenCodeAgent::plan` and
      `OpenCodeAgent::execute`, call `materialize_opencode_skills(ws)?`, build
      the config via `opencode_skills_config`, and pass it into
      `build_opencode_command`.
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, update the module-level doc
      comment (the `tracer slice` note at lines ~13–15 stating skills
      materialization D7 is "deferred-until-live ... not handled here") to
      reflect that D7 is now implemented.
- [x] Add a unit test `materialize_opencode_skills_extracts_required_skills`
      (mirroring the Codex test at `crates/ralphy-agent-codex/src/lib.rs:649`):
      assert `reviewer/SKILL.md`, `staged-plan/SKILL.md`, and
      `reviewer/scripts/audit.py` materialize under `.ralphy/skills`, that
      `.ralphy/.gitignore` is written, and that a second call is idempotent.
      (Fails to compile/pass before the new function exists; passes after.)
- [x] Add a unit test `opencode_skills_config_is_well_formed_json` that parses
      the `opencode_skills_config` output with `serde_json::from_str`, then
      asserts `["skills"]["paths"]` is a one-element array whose entry ends with
      the `.ralphy/skills` segment. (Fails before; passes after.)
- [x] Self-review: spawn the `reviewer` skill as an independent subagent over
      ONLY the commits made for this issue (#25) on this run's branch — not the
      whole branch, which carries earlier issues. Resolve every HIGH finding
      before finishing; if one cannot be fixed autonomously, record it under
      `## Notes & decisions` and block instead of declaring done.
- [x] Run `cargo fmt`, `cargo clippy`, and `cargo test -p ralphy-agent-opencode`
      (and a workspace `cargo build`); all pass with no new warnings.

## Notes & decisions
- Self-review found HIGH-1: `opencode_skills_config_is_well_formed_json` used a
  substring contains-check (`entry.contains("skills")`) that a non-conformant
  implementation could satisfy without injecting the correct path. Fixed in the
  same commit by replacing with `assert_eq!(PathBuf::from(entry), expected)` where
  `expected = dir.canonicalize().unwrap_or(dir)`. All 20 tests still pass.
