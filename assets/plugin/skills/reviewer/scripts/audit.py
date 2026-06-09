#!/usr/bin/env python3
"""audit.py — reconcile the LLM's `## Coverage` block against the fact pack.

Inputs:
  --coverage <path|->        coverage block (literal `-` reads stdin)
  --fact-pack <path>         JSON emitted by fact_pack.py
  --not-exercised <path|->   optional `not exercised:` block from the report
                             header. Used to enforce ADR-0003 F2 (one line per
                             command, no class bundling).
  --report <path|->          optional full report body (or any subset
                             containing `## Findings` / `## Verification`).
                             Material files whose path is cited in this body
                             are inferred as implicit-reviewed and removed
                             from `gap`. Coverage format F intentionally has
                             no positive marker; this flag lets the audit
                             observe the citations that already exist.

Output (stdout): one of
  audit: pass\nmaterial: N\nexcluded: N\nnot_reviewed: 0\ngap: none
  audit: gap\n...\ngap: <comma-list>
  audit: partial\n...\nnot_reviewed: N (<reasons>)\ngap: none

When pass/partial and the LLM declared `not-reviewed` covering more than
40% of the material set or more than 30 files, an auto-narrow trailer is
appended:

  scope-auto-narrowed: yes (N/M files; reason: <aggregate>)
         — narrowed-by-user-request: true|false|unspecified

When `narrowed-by-user-request` is `false` or `unspecified`, the
provocation block from ADR-0002 §A3 follows the trailer.

ADR-0003 format defects (informational; do not change exit code):
  - F1 glob in not-reviewed → `format-defect: glob-in-not-reviewed`
  - F1 category-empty       → `format-defect: category-empty`
  - F2 bundled not-exercised → `format-defect: bundled-not-exercised`

Exit codes: 0 on pass/partial; 2 on gap. Auto-narrow and format defects
are informational, not blocking — the verdict ceiling lives in the Stop
hook.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

AUTO_NARROW_PCT = 0.40
AUTO_NARROW_ABS = 30

PROVOCATION = (
    "You declared {n}/{m} material files as not-reviewed with reason "
    "'{reason}'. If the user explicitly requested a narrowed scope (e.g. "
    "'check only the brief acceptance criteria'), confirm by adding "
    "`narrowed-by-user-request: true` to your Coverage block. Otherwise, "
    "consider widening coverage or splitting `not-reviewed` reasons by "
    "file group."
)

GLOB_RE = re.compile(r"[*?\[\]]")
CATEGORY_RE = re.compile(r"^category\s*:\s*(.+)$", re.IGNORECASE)
# Heuristic for F2 bundling: an entry whose left side (before `—`/`-`/`:`
# blocker separator) names two or more check keywords joined by comma or
# `and`/`+`. Examples that match: "typecheck, lint, unit: infeasible",
# "typecheck and lint — side effects".
CHECK_TOKEN = (
    r"typecheck|tsc|lint|test|tests|unit|integration|contract|build|"
    r"docker|docker:build|release|format|fmt|ci|e2e"
)
BUNDLE_RE = re.compile(
    rf"^\s*(?:{CHECK_TOKEN})\s*(?:,|\band\b|\+)\s*(?:{CHECK_TOKEN})\b",
    re.IGNORECASE,
)


def parse_coverage_block(
    text: str,
) -> tuple[
    list[tuple[str, str]],
    list[tuple[str, str]],
    list[tuple[str, str]],
    list[str],
    str | None,
]:
    """Return (excluded, not_reviewed_paths, categories, glob_offenders, narrowed).

    `categories` is a list of (prefix, reason) entries from `category:` lines
    inside `not-reviewed:`. `glob_offenders` is the list of raw bullet entries
    that contained glob syntax.
    """
    excluded: list[tuple[str, str]] = []
    not_reviewed_paths: list[tuple[str, str]] = []
    categories: list[tuple[str, str]] = []
    glob_offenders: list[str] = []
    narrowed: str | None = None
    section: str | None = None
    flag_re = re.compile(r"^narrowed-by-user-request\s*:\s*(\S+)\s*$", re.IGNORECASE)

    for raw in text.splitlines():
        line = raw.rstrip()
        stripped = line.strip()
        if not stripped:
            continue
        flag_match = flag_re.match(stripped)
        if flag_match:
            value = flag_match.group(1).strip().lower().rstrip(".,;")
            if value in {"true", "false"}:
                narrowed = value
            continue
        low = stripped.lower().rstrip(":")
        if low == "excluded":
            section = "excluded"
            continue
        if low in {"not-reviewed", "not_reviewed", "notreviewed"}:
            section = "not_reviewed"
            continue
        if stripped.startswith("## "):
            if low.endswith("coverage"):
                section = None
                continue
            break
        if section and stripped.startswith("- "):
            entry = stripped[2:].strip()
            cat_match = CATEGORY_RE.match(entry)
            if section == "not_reviewed" and cat_match:
                prefix, reason = split_path_reason(cat_match.group(1).strip())
                if GLOB_RE.search(prefix):
                    glob_offenders.append(entry)
                else:
                    categories.append((prefix, reason))
                continue
            path, reason = split_path_reason(entry)
            if section == "not_reviewed" and GLOB_RE.search(path):
                glob_offenders.append(entry)
                continue
            if section == "excluded":
                excluded.append((path, reason))
            else:
                not_reviewed_paths.append((path, reason))
    return excluded, not_reviewed_paths, categories, glob_offenders, narrowed


def split_path_reason(entry: str) -> tuple[str, str]:
    """Split `path (reason)` into (path, reason). Reason may be empty."""
    if entry.endswith(")") and "(" in entry:
        idx = entry.rfind("(")
        path = entry[:idx].strip()
        reason = entry[idx + 1 : -1].strip()
        return path, reason
    return entry.strip(), ""


def aggregate_reasons(reasons: list[str]) -> str:
    seen: list[str] = []
    for reason in reasons:
        r = reason.strip() or "no reason"
        if r not in seen:
            seen.append(r)
    return "; ".join(seen) if seen else "no reason"


def find_cited_files(text: str, material: set[str]) -> set[str]:
    """Return the subset of `material` whose path is cited as a token in `text`.

    A path is "cited" when it appears with no word-character or `/` immediately
    before it, and no word-character or `.` immediately after it. This catches
    `Dockerfile:26`, `src/main.ts:282-346`, `` `tests/contract.test.ts` ``, and
    similar — but does not match `Dockerfile.dev` when only `Dockerfile` is
    material, nor `app/src/main.ts` when only `src/main.ts` is material.

    Used to infer implicit-reviewed files under Coverage format F (which has no
    positive marker): if the LLM cites a material file in `## Findings` or
    `## Verification`, count it as reviewed instead of leaking it into `gap`.
    """
    if not text or not material:
        return set()
    cited: set[str] = set()
    for path in material:
        if not path:
            continue
        pattern = re.compile(rf"(?<![\w/]){re.escape(path)}(?![\w.])")
        if pattern.search(text):
            cited.add(path)
    return cited


def files_under_prefix(prefix: str, material: set[str]) -> set[str]:
    """Return material files whose path starts with `prefix`.

    Prefix matching is literal. Trailing `/` is normalized so `src/adapters`
    and `src/adapters/` both match files under that directory. A bare
    file path matches itself.
    """
    p = prefix.rstrip("/")
    return {
        f
        for f in material
        if f == p or f.startswith(p + "/")
    }


def load_text(arg: str) -> str:
    if arg == "-":
        return sys.stdin.read()
    return Path(arg).read_text(encoding="utf-8")


def parse_not_exercised_block(text: str) -> list[str]:
    """Return the list of bullet entries (raw text after `- `).

    Accepts either the bare `not exercised:` block or a fragment containing
    it. Stops at a blank line followed by another field, or at the next
    Markdown header.
    """
    entries: list[str] = []
    in_block = False
    header_re = re.compile(r"^\s*not\s*exercised\s*:\s*(.*)$", re.IGNORECASE)
    for raw in text.splitlines():
        line = raw.rstrip()
        stripped = line.strip()
        if not in_block:
            m = header_re.match(line)
            if m:
                in_block = True
                tail = m.group(1).strip()
                # Inline form: "not exercised: none" or one-liner
                if tail and tail.lower() not in {"none", "n/a", "-"}:
                    # Treat the inline value as a single entry only if it
                    # looks like a bullet payload (rare); otherwise skip.
                    if tail.startswith("-"):
                        entries.append(tail.lstrip("- ").strip())
            continue
        # We are in the block.
        if stripped.startswith("## "):
            break
        if stripped.startswith("- "):
            entries.append(stripped[2:].strip())
            continue
        # A non-bullet, non-blank line that doesn't start with whitespace
        # likely starts a new header field. Stop.
        if stripped and not raw.startswith((" ", "\t")) and ":" in stripped:
            break
    return entries


def detect_bundled(entries: list[str]) -> list[str]:
    offenders: list[str] = []
    for entry in entries:
        # Split off the blocker separator: prefer em dash, then `:`, then `-`.
        head = entry
        for sep in ("—", " — ", ": ", " - "):
            if sep in entry:
                head = entry.split(sep, 1)[0]
                break
        if BUNDLE_RE.search(head.strip()):
            offenders.append(entry)
    return offenders


def maybe_emit_auto_narrow(
    not_reviewed_count: int,
    reasons: list[str],
    material_count: int,
    narrowed_flag: str | None,
) -> None:
    if not_reviewed_count == 0:
        return
    triggers = (
        (not_reviewed_count / max(material_count, 1)) > AUTO_NARROW_PCT
        or not_reviewed_count > AUTO_NARROW_ABS
    )
    if not triggers:
        return
    reason = aggregate_reasons(reasons)
    flag_value = narrowed_flag if narrowed_flag in {"true", "false"} else "unspecified"
    print(
        f"scope-auto-narrowed: yes ({not_reviewed_count}/{material_count} files; reason: {reason})"
    )
    print(f"       — narrowed-by-user-request: {flag_value}")
    if flag_value != "true":
        print()
        print(PROVOCATION.format(n=not_reviewed_count, m=material_count, reason=reason))


SKIP_CLAUSES = {
    "skip:trivial-diff",
    "skip:docs-only",
    "skip:verifier-infeasible",
    "skip:no-tests-touched",
    "skip:user-narrowed",
}

MANDATORY_SUBAGENTS = ("defect-hunter", "test-auditor", "verifier")


def parse_invoked_line(report_text: str) -> dict[str, int] | None:
    """Parse `invoked: defect-hunter (N), test-auditor (N), verifier (N), scout (N)`
    from ## Notes. Returns dict of name->count, or None if line absent.
    `invoked: none` returns all-zero dict."""
    import re
    m = re.search(r"^\s*[-*]?\s*invoked:\s*(.+)$", report_text, re.MULTILINE)
    if not m:
        return None
    body = m.group(1).strip()
    if body.lower() == "none":
        return {name: 0 for name in MANDATORY_SUBAGENTS + ("scout",)}
    counts: dict[str, int] = {}
    for entry in re.finditer(r"([a-z-]+)\s*\((\d+)\)", body):
        counts[entry.group(1)] = int(entry.group(2))
    return counts


def parse_skip_clauses(report_text: str) -> dict[str, str]:
    """Find lines like `skip:<clause> — <subagent>: <reason>` in ## Notes.
    Returns dict subagent->clause. Subagent must be one of MANDATORY_SUBAGENTS."""
    import re
    skips: dict[str, str] = {}
    for m in re.finditer(
        r"(skip:[a-z-]+)\s*[—-]+\s*([a-z-]+)\s*:", report_text
    ):
        clause, subagent = m.group(1), m.group(2)
        if clause in SKIP_CLAUSES and subagent in MANDATORY_SUBAGENTS:
            skips[subagent] = clause
    return skips


def threshold_crossed(
    material: set[str], not_exercised_text: str
) -> tuple[bool, list[str]]:
    """Return (crossed, reasons). Mirrors SKILL.md threshold definition."""
    import re
    reasons: list[str] = []
    if len(material) > 10:
        reasons.append(f"material_set={len(material)} > 10")
    infra_pat = re.compile(
        r"(^|/)(Dockerfile|docker-compose|\.github/workflows/|\.gitlab-ci|tests?/)"
    )
    infra_hits = [f for f in material if infra_pat.search(f)]
    if infra_hits:
        reasons.append(f"infra/test paths touched ({len(infra_hits)} files)")
    return (bool(reasons), reasons)


def check_subagent_mandate(
    report_text: str | None,
    material: set[str],
    not_exercised_text: str,
) -> list[str]:
    """Return list of format-defect lines if mandate is violated. Empty if OK."""
    crossed, reasons = threshold_crossed(material, not_exercised_text)
    if not crossed:
        return []
    if report_text is None:
        return [
            "format-defect: subagent-mandate-unverifiable",
            "  - threshold crossed but --report not supplied; cannot verify invoked: line",
            f"  - threshold reasons: {'; '.join(reasons)}",
        ]
    invoked = parse_invoked_line(report_text)
    if invoked is None:
        return [
            "format-defect: subagent-invoked-line-missing",
            "  - threshold crossed; ## Notes must contain `invoked: ...` line",
            f"  - threshold reasons: {'; '.join(reasons)}",
        ]
    skips = parse_skip_clauses(report_text)
    offenders: list[str] = []
    for name in MANDATORY_SUBAGENTS:
        if invoked.get(name, 0) >= 1:
            continue
        if name in skips:
            continue
        offenders.append(name)
    if not offenders:
        return []
    out = ["format-defect: subagent-skip-uncited"]
    for name in offenders:
        out.append(
            f"  - {name}: count=0 and no `skip:<clause> — {name}:` line found "
            f"in ## Notes (threshold crossed: {'; '.join(reasons)})"
        )
    return out


def emit_format_defects(
    glob_offenders: list[str],
    category_empty: list[tuple[str, str]],
    bundled: list[str],
    subagent_defects: list[str] | None = None,
) -> None:
    if not (glob_offenders or category_empty or bundled or subagent_defects):
        return
    print()
    if glob_offenders:
        print("format-defect: glob-in-not-reviewed")
        for offender in glob_offenders:
            print(f"  - {offender}")
    if category_empty:
        print("format-defect: category-empty")
        for prefix, reason in category_empty:
            label = f"{prefix} ({reason})" if reason else prefix
            print(f"  - category: {label}")
    if bundled:
        print("format-defect: bundled-not-exercised")
        for offender in bundled:
            print(f"  - {offender}")
    if subagent_defects:
        for line in subagent_defects:
            print(line)


def main() -> int:
    ap = argparse.ArgumentParser(description="Reconcile LLM coverage block against fact pack.")
    ap.add_argument("--coverage", required=True, help="Path to coverage block, or '-' for stdin")
    ap.add_argument("--fact-pack", required=True, help="Path to fact_pack.py JSON output")
    ap.add_argument(
        "--not-exercised",
        default=None,
        help="Optional path to the report's `not exercised:` block, or '-' for stdin",
    )
    ap.add_argument(
        "--report",
        default=None,
        help=(
            "Optional path to the full report body (or `## Findings` + "
            "`## Verification` excerpt), or '-' for stdin. Material files "
            "cited in this body are inferred as implicit-reviewed and removed "
            "from `gap`."
        ),
    )
    args = ap.parse_args()

    coverage_text = load_text(args.coverage)
    fact_pack = json.loads(Path(args.fact_pack).read_text(encoding="utf-8"))

    material: set[str] = set(fact_pack.get("material_files", []))
    fact_excluded: set[str] = {e["path"] for e in fact_pack.get("excluded_files", [])}

    (
        excluded_entries,
        not_reviewed_paths,
        categories,
        glob_offenders,
        narrowed_flag,
    ) = parse_coverage_block(coverage_text)
    claimed_excluded = {p for p, _ in excluded_entries}

    explicit_paths = {p for p, _ in not_reviewed_paths}
    category_covered: set[str] = set()
    category_lines: list[str] = []
    category_empty: list[tuple[str, str]] = []
    for prefix, reason in categories:
        matched = files_under_prefix(prefix, material)
        if not matched:
            category_empty.append((prefix, reason))
        category_covered |= matched
        label = f"{prefix} ({reason})" if reason else prefix
        category_lines.append(f"category: {label} — {len(matched)} files under prefix")

    explicit_not_reviewed = explicit_paths | category_covered

    cited: set[str] = set()
    if args.report:
        report_text = load_text(args.report)
        cited = find_cited_files(report_text, material)

    gap = sorted(
        material - explicit_not_reviewed - claimed_excluded - cited - {""}
    )

    not_reviewed_count = len(explicit_not_reviewed)
    reasons_list = [r for _, r in not_reviewed_paths] + [r for _, r in categories]
    not_reviewed_summary_parts: list[str] = []
    for p, r in not_reviewed_paths:
        not_reviewed_summary_parts.append(f"{p}: {r or 'no reason'}")
    for prefix, r in categories:
        not_reviewed_summary_parts.append(f"category {prefix}: {r or 'no reason'}")
    not_reviewed_summary = ", ".join(not_reviewed_summary_parts)

    bundled: list[str] = []
    ne_text_raw = ""
    if args.not_exercised:
        ne_text_raw = load_text(args.not_exercised)
        ne_entries = parse_not_exercised_block(ne_text_raw)
        # If the file does not contain a `not exercised:` header, treat each
        # bullet line as an entry — supports passing the bare block.
        if not ne_entries:
            ne_entries = [
                line.strip()[2:].strip()
                for line in ne_text_raw.splitlines()
                if line.strip().startswith("- ")
            ]
        bundled = detect_bundled(ne_entries)

    report_text_for_mandate = report_text if args.report else None
    subagent_defects = check_subagent_mandate(
        report_text_for_mandate, material, ne_text_raw
    )

    out_lines = [
        f"material: {len(material)}",
        f"excluded: {len(fact_excluded)}",
    ]
    if cited:
        out_lines.append(f"cited_in_report: {len(cited)}")

    if gap:
        print("audit: gap")
        for line in out_lines:
            print(line)
        if not_reviewed_count:
            print(f"not_reviewed: {not_reviewed_count} ({not_reviewed_summary})")
        else:
            print("not_reviewed: 0")
        for line in category_lines:
            print(line)
        print("gap: " + ", ".join(gap))
        emit_format_defects(glob_offenders, category_empty, bundled, subagent_defects)
        return 2

    if not_reviewed_count:
        print("audit: partial")
        for line in out_lines:
            print(line)
        print(f"not_reviewed: {not_reviewed_count} ({not_reviewed_summary})")
        for line in category_lines:
            print(line)
        print("gap: none")
        maybe_emit_auto_narrow(
            not_reviewed_count, reasons_list, len(material), narrowed_flag
        )
        emit_format_defects(glob_offenders, category_empty, bundled, subagent_defects)
        return 2 if subagent_defects else 0

    print("audit: pass")
    for line in out_lines:
        print(line)
    print("not_reviewed: 0")
    for line in category_lines:
        print(line)
    print("gap: none")
    emit_format_defects(glob_offenders, category_empty, bundled, subagent_defects)
    return 2 if subagent_defects else 0


if __name__ == "__main__":
    sys.exit(main())
