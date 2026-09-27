#!/usr/bin/env python3
"""CI gate for every stage of issue-485-test-suite-cleanup.

Usage: python docs/plans/issue-485-test-suite-cleanup-verify-gate.py [--ui] [--ids ID1,ID2,...]

--ui   also runs the node UI tests and the ui-copy lint.
--ids  checks that each ID has a row in section 7 of the audit report.
"""
import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from _verify import V, _resolve  # noqa: E402

REPORT = "docs/audit-tests-2026-09-27.md"
# Finding IDs as the audit report writes them: P0-1, A-1, D-1, C-A1, C-B1, K-1, S-1, J-1, G-1.
ID_RE = re.compile(r"(?<![\w-])(P0-\d+|C-[AB]\d+|[ADKSJG]-\d+)(?![\w-])")


def report_text() -> str:
    return _resolve(REPORT).read_text(encoding="utf-8")


def ledger_ids(text: str) -> set[str]:
    """IDs named in the first cell of a row of the section-7 status table."""
    section = text.split("## 7.", 1)[1] if "## 7." in text else ""
    found = set()
    for line in section.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if line.lstrip().startswith("|") and len(cells) >= 2:
            found.update(ID_RE.findall(cells[0]))
    return found


def finding_ids(text: str) -> set[str]:
    """Every ID defined in sections 2 to 4 of the report."""
    body = text.split("## 2.", 1)[1].split("## 5.", 1)[0]
    return set(ID_RE.findall(body))


def check_ids(ids: list[str]) -> None:
    have = ledger_ids(report_text())
    missing = [i for i in ids if i not in have]
    V._record(f"section 7 rows for {len(ids)} IDs", not missing, "missing: " + ", ".join(missing))


def run_ci_gate(ui: bool) -> None:
    V.run_gate("cargo fmt --all --check", gate="fmt")
    V.run_gate("cargo clippy --workspace --all-targets -- -D warnings", gate="clippy", timeout=3600)
    V.run_gate("cargo nextest run --workspace", gate="nextest", timeout=3600)
    V.run_gate("cargo test --workspace --doc", gate="doctest", timeout=3600)
    V.run_gate("cargo run -q -p xtask -- changelog --check", gate="changelog")
    if ui:
        V.run_gate("node --test crates/ralphy-daemon/ui-tests", gate="ui-tests")
        V.run_gate("cargo run -q -p xtask -- ui-copy --check", gate="ui-copy")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ui", action="store_true")
    ap.add_argument("--ids", default="")
    args = ap.parse_args()
    if args.ids:
        check_ids([i.strip() for i in args.ids.split(",") if i.strip()])
    run_ci_gate(args.ui)
    return V.summarize()


if __name__ == "__main__":
    sys.exit(main())
