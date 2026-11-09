#!/usr/bin/env python3
"""Hold the run to every row of the references it read.

A reference is a set of tables: each row is a check, a tool, a rule group or a
knob the target names. Skipping a row fails nothing by itself; this script
makes the skip fail.

  rows.py list REF...           # the Coverage table to fill: one line per row
  rows.py check EVIDENCE REF... # fails when a row has no verdict in EVIDENCE
  rows.py keys REF... --in FILE...   # the target config keys no FILE names

The plan's evidence (`lint-evidence.md`) carries a `## Coverage` (or `### Coverage`) section with a table `| Row | Verdict |`. A
verdict is one of:

  on                        the check runs in the gate today (`on — <note>` too)
  sites N                   the rule has N sites; its phase is in the debt table
  phase N                   a phase of the plan delivers it
  closed by <check>         an exit and the check that closes it
  open                      an exit no tool closes; a human reads it (`open — <note>` too)
  decision — <Dn>           waiting on a decision of the plan
  n/a — <why>               the repo has nothing it applies to
  out — <why>               a row of a "Do not turn on" table, kept off
  not measured — <why>      a knob whose measure needs what the run lacks

`check` reports the rows with no line, the lines with no valid verdict, and
the lines that name a row no reference has (a reference changed).

`keys` reads the code blocks of the references (the target configs: TOML, INI,
JSON with comments, YAML) and prints every key, lint or linter they set that
none of the FILEs names: pass the repo's real configs and the plan. A key the
run took in is in a config; one it left is in the plan with its reason.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

_SEP = re.compile(r"^\|[\s|:-]+\|\s*$")
_VERDICT = re.compile(
    r"^(on|open|on — .+|open — .+|sites \d[\d,]*.*|phase \d+.*|closed by .+|decision — .+|n/a — .+|out — .+|not measured — .+)$"
)


_CELL = re.compile(r"(?<!\\)\|")


def cells(line: str) -> list[str]:
    """The cells of a table row; an escaped pipe stays inside its cell."""
    return _CELL.split(line.strip().strip("|"))


def clean(cell: str) -> str:
    cell = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", cell)
    cell = cell.replace("`", "").replace("**", "")
    return re.sub(r"\s+", " ", cell).strip()


def rows_of(path: Path) -> list[str]:
    stem = "knobs" if path.name == "SKILL.md" else path.stem
    lines = path.read_text(encoding="utf-8-sig").splitlines()
    out: list[str] = []
    i = 0
    while i < len(lines):
        if lines[i].startswith("|") and i + 1 < len(lines) and _SEP.match(lines[i + 1]):
            head = clean(cells(lines[i])[0])
            i += 2
            while i < len(lines) and lines[i].startswith("|"):
                first = clean(cells(lines[i])[0])
                rid = f"{stem}: {first}"
                if rid in out:
                    rid = f"{stem}: {first} ({head})"
                if first:
                    out.append(rid)
                i += 1
        else:
            i += 1
    return out


def coverage(plan: Path) -> dict[str, str]:
    text = plan.read_text(encoding="utf-8-sig")
    m = re.search(r"^#{2,3} Coverage\s*$(.*?)(?=^#{1,3} |\Z)", text, re.M | re.S)
    if not m:
        return {}
    got: dict[str, str] = {}
    for line in m.group(1).splitlines():
        if not line.startswith("|") or _SEP.match(line):
            continue
        row = [clean(c) for c in cells(line)]
        if len(row) >= 2 and row[0] not in ("Row", ""):
            got[row[0]] = row[1]
    return got


_BLOCK = re.compile(r"^```(toml|ini|jsonc|json|yaml)\n(.*?)^```", re.M | re.S)
_KEY = {
    "toml": re.compile(r'^\s*"?([A-Za-z][\w./-]*)"?(?:\.msg)?\s*='),
    "ini": re.compile(r"^\s*([A-Za-z][\w.-]*)\s*="),
    "jsonc": re.compile(r'^\s*"([A-Za-z][\w/@.-]*)"\s*:'),
    "yaml": re.compile(r"^\s*(?:-\s+)?([A-Za-z][\w.-]*)\s*(?::|$|#)"),
}
_KEY["json"] = _KEY["jsonc"]


def keys_of(path: Path) -> list[str]:
    out: list[str] = []
    for m in _BLOCK.finditer(path.read_text(encoding="utf-8-sig")):
        for line in m.group(2).splitlines():
            if line.lstrip().startswith(("#", "//", "{", "[")):
                continue
            k = _KEY[m.group(1)].match(line)
            if k and k.group(1) not in out:
                out.append(k.group(1))
    return out


def keys(argv: list[str]) -> int:
    if "--in" not in argv:
        print(__doc__, file=sys.stderr)
        return 2
    i = argv.index("--in")
    refs, files = [Path(a) for a in argv[:i]], [Path(a) for a in argv[i + 1:]]
    text = "\n".join(f.read_text(encoding="utf-8-sig", errors="replace") for f in files)
    missing = []
    for ref in refs:
        for k in keys_of(ref):
            name = k.split("/")[-1]
            if not re.search(r"(?<![\w-])" + re.escape(name).replace("_", "[_-]") + r"(?![\w-])", text):
                missing.append(f"{ref.stem}: {k}")
    for m in missing:
        print(f"named nowhere: {m}")
    print(f"{sum(len(keys_of(r)) for r in refs)} keys, {len(missing)} named in no config and not in the plan")
    return 1 if missing else 0


def main() -> int:
    if len(sys.argv) >= 2 and sys.argv[1] == "keys":
        return keys(sys.argv[2:])
    if len(sys.argv) < 3 or sys.argv[1] not in ("list", "check"):
        print(__doc__, file=sys.stderr)
        return 0 if sys.argv[1:2] in (["-h"], ["--help"]) else 2
    if sys.argv[1] == "list":
        print("| Row | Verdict |\n|---|---|")
        for ref in sys.argv[2:]:
            for rid in rows_of(Path(ref)):
                print(f"| {rid} | |")
        return 0
    plan, refs = Path(sys.argv[2]), [Path(r) for r in sys.argv[3:]]
    want = [rid for r in refs for rid in rows_of(r)]
    got = coverage(plan)
    if not got:
        print(f"{plan}: no `## Coverage` table")
        return 1
    missing = [r for r in want if r not in got]
    bad = [f"{r}: {got[r]!r}" for r in want if r in got and not _VERDICT.match(got[r])]
    stale = [r for r in got if r not in want]
    for label, items in (("no line", missing), ("no valid verdict", bad), ("not a row of the references", stale)):
        for item in items:
            print(f"{label}: {item}")
    print(f"{len(want)} rows, {len(want) - len(missing) - len(bad)} with a verdict")
    return 1 if missing or bad or stale else 0


if __name__ == "__main__":
    sys.exit(main())
