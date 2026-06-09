# Verify scripts

## Trigger

Generate `docs/plans/<plan-slug>-verify-stage-N.py` (or `-verify-e2e.py`) when **any** condition holds:
- The stage has **≥4 distinct shell commands** in its `**Verification:**` block.
- The stage has **≥2 grep-based invariant assertions** (e.g., "must return 0 matches for `OldLib`", "must still find `console.error`").
- The stage uses **≥2 `_verify` primitives** (`assert_clean_tree`, `assert_commit_present`, `assert_only_files_touched`, `assert_report_exists`, `assert_grep_zero`).

**When in doubt, generate the script** — it costs ~20 lines and is reusable across retries. Inline is only appropriate for simple two-command gates (`bun test` + `bun run build`).

## Why Python, not bash

Scripts must run unchanged on Linux, macOS, and Windows native (no WSL/Git-Bash dependency). Python 3 stdlib is the cross-platform denominator and is available on every dev box.

## Vendoring

The primitives live at `~/.claude/skills/staged-plan/lib/verify.py` (the canonical copy) and are vendored into each repo at `<repo>/docs/plans/_verify.py` as part of the **Plan landing commit** (Phase 1.5), before Phase 2 begins. Generated stage scripts import from the vendored copy, so any executor (Claude, Codex, human dev) can run `python docs/plans/<slug>-verify-stage-N.py` without the skill being installed.

## Logs policy

`run_gate()` always writes a timestamped log to `<repo>/docs/plans/logs/<prefix>-<ts>.log`. These are local evidence artifacts, **not versioned**: the Plan landing commit adds `docs/plans/logs/` to `.gitignore`. Reports (committed alongside each stage) capture deviations and judgments for PR review; raw logs are forensic-only and may grow large.

## Generated script shape

```python
#!/usr/bin/env python3
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent))
from _verify import V

V.assert_clean_tree()
V.assert_commit_present(r"^feat: add B-backed impl")
V.assert_only_files_touched(["src/y_v2.ts"], base_sha="HEAD~1")
V.run_gate("bun run build")
V.run_gate("bun test src/y_v2.test.ts")
V.assert_grep_zero(r"\bimport.*from ['\"]lib-a['\"]", ["src/"])
V.assert_report_exists("docs/plans/migration-x-stage-1-report.md")
sys.exit(V.summarize())
```
