# Minimal example — Migration X

A reference of what a fully-filled staged plan looks like for a small migration track. Use as a template when uncertain about layout; not a live plan.

```
# Migration X - Staged Execution Plan

## Execution model (READ FIRST)
Staged subagent execution. One subagent per stage via the executor adapter, in
order, foreground, inherited model. Verify green (build + commit SHA) between
stages.

## Execution policy
- Mode: autonomous
- Commit authorization: per-stage-direct
- On red: auto-retry-up-to-2
- Working-tree policy: clean-required
- Reviewer: none

## Executor adapter
- Claude Code: `Agent` tool, `subagent_type: general-purpose`, foreground.
- Codex: execute each Hand-off prompt inline in a fresh context.

## Context
Migrate module Y from lib A to lib B. 7 files, 3 public callsites.

## Global conventions
- Build: `<cmd>`
- Tests: `<cmd>`
- Commit style: one per stage, trailer Co-Authored-By.
- Staging: explicit paths only.

## Stage 0 - Pre-flight
Record HEAD + git status. Apply clean-required policy. Run build + tests on
HEAD; abort if red. (`_verify.py` was vendored via the Plan landing commit
before this stage.)

## Stage 1 - Add B-backed implementation alongside A
**Files:** `src/y_v2.ext` (new)
**Order:** add file, export, build, commit.
**Hand-off for Stage 1:** <self-contained prompt>

## Stage 2 - Port callsites
**Files:** `src/caller1.ext`, `src/caller2.ext`, `src/caller3.ext`
**Order:** swap imports, build, tests, commit.
**Hand-off for Stage 2:** <self-contained prompt>

## Stage 3 - Remove A-backed implementation
**Files:** `src/y.ext` (delete), `Cargo.toml` / `package.json` (drop dep)
**Order:** delete, remove dep, build, `grep "<A>"` returns zero, commit.

## End-to-end verification
Full test suite; grep for residual references to A.
```
