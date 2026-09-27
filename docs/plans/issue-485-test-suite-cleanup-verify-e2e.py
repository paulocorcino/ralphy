#!/usr/bin/env python3
"""End-to-end check for issue-485-test-suite-cleanup.

Usage: python docs/plans/issue-485-test-suite-cleanup-verify-e2e.py [--ledger-only]

Checks that every finding ID in sections 2-4 of the audit report has a row in
section 7, and that no Commit cell still says `stage N`. Without --ledger-only it
also runs the full CI gate, UI tests included.
"""
import importlib.util
import re
import sys
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from _verify import V  # noqa: E402

spec = importlib.util.spec_from_file_location("gate", HERE / "issue-485-test-suite-cleanup-verify-gate.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def main() -> int:
    ledger_only = "--ledger-only" in sys.argv[1:]
    text = gate.report_text()
    missing = sorted(gate.finding_ids(text) - gate.ledger_ids(text))
    V._record("every finding ID has a section 7 row", not missing, "missing: " + ", ".join(missing))
    if not ledger_only:
        section = text.split("## 7.", 1)[1]
        stale = [ln for ln in section.splitlines() if re.search(r"\|\s*stage \d+\s*\|", ln)]
        V._record("every Commit cell holds a SHA", not stale, "\n".join(stale[:5]))
        gate.run_ci_gate(ui=True)
    return V.summarize()


if __name__ == "__main__":
    sys.exit(main())
