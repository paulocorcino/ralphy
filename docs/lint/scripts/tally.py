#!/usr/bin/env python3
"""Count lint sites per rule from a tool's machine-readable output.

A site is one (rule, file, line, column). Duplicates are dropped: clippy reports
a site once per target that compiles the file, and a macro can repeat it; ruff
can report one rule twice at one position (PT018 on `assert a and b and c`).

  tally.py FORMAT OUTPUT [--json OUT.json] [--scope-depth N]
  tally.py diff A.json B.json                  # sites in B that are not in A
  tally.py curve FORMAT OUTPUT --rule RULE [--at 5,8,10,12,15,20]
  tally.py pattern REGEX [PATH...] --ext .rs,.ts [--expect N] [--scope-depth N]
  tally.py sizes [PATH...] --ext .py,.ts [--lines 300,400,500,700] [--width 80,100,120]
  tally.py added FORMAT OUTPUT --base REF [--ignore RULE,RULE]

FORMAT is one of: clippy, oxlint, eslint, ruff, mypy, pyright, golangci, tsc.
OUTPUT is the file the tool wrote (`-` for stdin).

  clippy    cargo clippy --message-format=json            (JSON lines)
  oxlint    oxlint --format json
  eslint    eslint -f json
  ruff      ruff check --output-format json
  mypy      mypy -O json                                  (JSON lines)
  pyright   pyright --outputjson
  golangci  golangci-lint run --output.json.path=FILE     (v2)
  tsc       tsc --noEmit --pretty false                   (text)
  vulture   vulture <paths>                               (text)
  knip      knip --reporter json
  astgrep   ast-grep scan --json=stream <paths>          (a clean run prints nothing)
  jscpd     jscpd --reporters json --output DIR           (DIR/jscpd-report.json; one
            site per clone, at its first file; pass --prefix with the scanned folder)

--expect N makes the count a ratchet: it fails when the total is above N (new
debt) and when it is below N (the change that removed sites lowers N, so the
gain is kept).

--scope-depth N groups each rule by the first N path components (a crate,
a package, a directory), to cut the work into slices.

`diff` splits test sites from production when the tool cannot: tally the run
over production code only (A) and the run over every target (B); the diff is
the test sites.

`curve` gives the debt of a numeric limit at several thresholds from one run.
Run the tool with that limit set very low (complexity 1, arguments 0), so it
reports every function with its value; `curve` reads the value from each
message and counts the sites above each threshold. The messages it reads:
"(13 > 10)" (ruff), "(23/10)" and "(7/5)" (clippy), "complexity of 13"
(oxlint, eslint), "complexity 23 of" (gocognit), "but got 7" (revive),
"too many parameters (6)" (oxlint and eslint `max-params`).

`pattern` counts the lines of the files tracked by git (under PATH, with an
extension of --ext) that match REGEX: the ratchet of a forbidden pattern no
linter has, such as a `TODO` in Rust or a suppression marker. With --expect it
fails above and below N, as the plain mode does.

`sizes` gives the debt of a file-size limit and a line-width limit: the files
tracked by git under PATH with an extension of --ext, the number of their
non-blank lines, and the lines wider than each width.

The plain mode adds the columns `safe` and `unsafe` when the tool reports its
own fixes (ruff, clippy): the sites its fix command rewrites, apart from the
ones that need a hand.

`added` holds new code to the whole target while the debt is paid: it reads
the output of a draft run (every rule of the target on, the debt rules too)
and fails on each site that sits on a line the change adds against REF (the
merge base: `git merge-base HEAD origin/main`; the working tree counts, so it
runs before a commit too). A site on an old line is the plan's; a site on a
new line is the change's. --ignore leaves out rules that report a line the
change did not write (complexity at the `def`: a baseline holds those).

Warnings on stderr mean the count is incomplete: the tool reported a parse or
build error, so part of the code was never checked.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from typing import Iterable, Iterator

Site = tuple[str, str, int, int]  # rule, file, line, column
Hit = tuple[str, str, int, int, str]  # a site and its message
# A parser may yield a sixth field: "safe" when the tool's own fix keeps the
# behavior, "unsafe" when it may change it, "" when the tool has no fix.


def warn(msg: str) -> None:
    print(f"warning: {msg}", file=sys.stderr)


def norm_path(p: str) -> str:
    p = p.replace("\\", "/")
    cwd = os.getcwd().replace("\\", "/").rstrip("/") + "/"
    if p.lower().startswith(cwd.lower()):
        p = p[len(cwd):]
    if p.startswith("./"):
        p = p[2:]
    return p


def json_lines(text: str) -> Iterator[dict]:
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def parse_clippy(text: str) -> Iterable[Hit]:
    for m in json_lines(text):
        if m.get("reason") == "build-finished" and not m.get("success", True):
            warn("the build did not finish: crates that failed were not linted")
        if m.get("reason") != "compiler-message":
            continue
        msg = m["message"]
        code = (msg.get("code") or {}).get("code")
        text_ = msg.get("message", "")
        if not code:
            if msg.get("level") == "error":
                warn(f"compile error with no code: {text_[:100]}")
            continue
        if re.fullmatch(r"E\d{4}", code):
            warn(f"compile error {code}: the crate did not build, its lints are missing")
            continue
        spans = [s for s in msg.get("spans", []) if s.get("is_primary")] or msg.get("spans", [])
        if not spans:
            yield code, "(no file)", 0, 0, text_
            continue
        s = spans[0]
        yield code, norm_path(s["file_name"]), s["line_start"], s["column_start"], text_, clippy_fix(msg)


_CLIPPY_SAFE = {"MachineApplicable"}
_CLIPPY_UNSAFE = {"MaybeIncorrect", "HasPlaceholders"}


def clippy_fix(msg: dict) -> str:
    kinds = {sp.get("suggestion_applicability")
             for m in [msg, *msg.get("children", [])] for sp in m.get("spans", [])}
    return "safe" if kinds & _CLIPPY_SAFE else "unsafe" if kinds & _CLIPPY_UNSAFE else ""


def parse_oxlint(text: str) -> Iterable[Hit]:
    data = json.loads(text)
    for d in data.get("diagnostics", []):
        code = d.get("code") or "(parse error)"
        if code == "(parse error)":
            warn(f"diagnostic with no rule in {d.get('filename')}: {d.get('message', '')[:100]}")
        labels = d.get("labels") or [{}]
        span = labels[0].get("span", {})
        yield (code, norm_path(d.get("filename", "(no file)")), span.get("line", 0),
               span.get("column", 0), d.get("message", ""))


def parse_eslint(text: str) -> Iterable[Hit]:
    for f in json.loads(text):
        for m in f.get("messages", []):
            rule = m.get("ruleId")
            if rule is None:
                warn(f"{f.get('filePath')}: {m.get('message', '')[:100]}")
                rule = "(parse error)" if m.get("fatal") else "(no rule)"
            yield rule, norm_path(f["filePath"]), m.get("line", 0), m.get("column", 0), m.get("message", "")


def parse_ruff(text: str) -> Iterable[Hit]:
    for d in json.loads(text):
        code = d.get("code")
        if code is None:
            warn(f"{d.get('filename')}: {d.get('message', '')[:100]}")
            code = "(syntax error)"
        loc = d.get("location") or {}
        kind = (d.get("fix") or {}).get("applicability", "")
        yield (code, norm_path(d["filename"]), loc.get("row", 0), loc.get("column", 0),
               d.get("message", ""), kind if kind in ("safe", "unsafe") else "")


def parse_mypy(text: str) -> Iterable[Hit]:
    for d in json_lines(text):
        if d.get("severity") == "note":
            continue
        code = d.get("code") or "(no code)"
        if code == "syntax":
            warn(f"{d.get('file')}: syntax error, the file was not checked")
        yield code, norm_path(d["file"]), d.get("line", 0), d.get("column", 0), d.get("message", "")


def parse_pyright(text: str) -> Iterable[Hit]:
    data = json.loads(text)
    for d in data.get("generalDiagnostics", []):
        if d.get("severity") == "information":
            continue
        start = (d.get("range") or {}).get("start", {})
        yield (
            d.get("rule") or "(no rule)",
            norm_path(d.get("file", "(no file)")),
            start.get("line", 0) + 1,
            start.get("character", 0) + 1,
            d.get("message", ""),
        )


# staticcheck (SA1019, ST1003, QF1001, S1000) and gosec (G104) put their own
# check id at the start of the text: count those per id, not per linter.
_GO_CHECK_ID = re.compile(r"^([A-Z]{1,3}\d{3,4})\b")
# revive (`argument-limit: ...`) and gocritic (`ifElseChain: ...`) start with
# the name of their rule.
_GO_RULE_NAME = re.compile(r"^([A-Za-z][\w-]+): ")
_GO_NAMED_RULES = {"revive", "gocritic"}


def parse_golangci(text: str) -> Iterable[Hit]:
    data = json.loads(text)
    for i in data.get("Issues") or []:
        rule = i.get("FromLinter", "(no linter)")
        pattern = _GO_RULE_NAME if rule in _GO_NAMED_RULES else _GO_CHECK_ID
        m = pattern.match(i.get("Text", ""))
        if m:
            rule = f"{rule}:{m.group(1)}"
        if rule == "typecheck":
            warn(f"typecheck: {i.get('Text', '')[:100]} (the package was not linted)")
        pos = i.get("Pos") or {}
        yield (rule, norm_path(pos.get("Filename", "(no file)")), pos.get("Line", 0),
               pos.get("Column", 0), i.get("Text", ""))


_TSC_LINE = re.compile(r"^(.+?)\((\d+),(\d+)\): error (TS\d+): ?(.*)")


def parse_tsc(text: str) -> Iterable[Hit]:
    for line in text.splitlines():
        m = _TSC_LINE.match(line)
        if m:
            yield m.group(4), norm_path(m.group(1)), int(m.group(2)), int(m.group(3)), m.group(5)


def parse_jscpd(text: str) -> Iterable[Hit]:
    """One site per clone, at its first file; the message carries its size."""
    for c in json.loads(text).get("duplicates") or []:
        first, second = c.get("firstFile") or {}, c.get("secondFile") or {}
        other = f"{norm_path(second.get('name', '?'))}:{second.get('start', 0)}"
        # A block copied to several places makes several clones that start at
        # one place: the column slot holds a stable id of the other copy.
        yield (
            f"jscpd:{c.get('format', '?')}",
            norm_path(first.get("name", "(no file)")),
            first.get("start", (first.get("startLoc") or {}).get("line", 0)),
            int(hashlib.sha1(other.encode()).hexdigest()[:8], 16),
            f"{c.get('tokens', 0)} tokens, {c.get('lines', 0)} lines, "
            f"also at {other}",
        )


_VULTURE = re.compile(r"^(.+?):(\d+): (unused [a-z ]+?|unreachable code|unsatisfiable \w+ condition)\b(.*)$")


def parse_vulture(text: str) -> Iterable[Hit]:
    for line in text.splitlines():
        m = _VULTURE.match(line.strip())
        if m:
            yield f"vulture:{m.group(3)}", norm_path(m.group(1)), int(m.group(2)), 0, m.group(4).strip()


def parse_knip(text: str) -> Iterable[Hit]:
    """knip --reporter json: one site per unused file, export, type or dependency."""
    for issue in json.loads(text).get("issues") or []:
        path = norm_path(issue.get("file", "(no file)"))
        for kind, items in issue.items():
            if kind == "file" or not isinstance(items, list):
                continue
            for item in items:
                item = item if isinstance(item, dict) else {"name": str(item)}
                yield (f"knip:{kind}", path, item.get("line", 0), item.get("col", 0),
                       str(item.get("name", "")))


def parse_astgrep(text: str) -> Iterable[Hit]:
    """ast-grep scan --json=stream (one object per line), or --json (one array)."""
    text = text.strip()
    items = json.loads(text) if text.startswith("[") else [json.loads(l) for l in text.splitlines() if l.strip()]
    for x in items:
        start = x.get("range", {}).get("start", {})
        yield (f"ast-grep:{x.get('ruleId', '?')}", norm_path(x.get("file", "(no file)")),
               start.get("line", -1) + 1, start.get("column", -1) + 1, x.get("message", ""))


PARSERS = {
    "clippy": parse_clippy,
    "oxlint": parse_oxlint,
    "eslint": parse_eslint,
    "ruff": parse_ruff,
    "mypy": parse_mypy,
    "pyright": parse_pyright,
    "golangci": parse_golangci,
    "tsc": parse_tsc,
    "jscpd": parse_jscpd,
    "vulture": parse_vulture,
    "knip": parse_knip,
    "astgrep": parse_astgrep,
}

_VALUE = [
    re.compile(r"\((\d+)\s*[>/]\s*\d+\)"),
    re.compile(r"complexity of (\d+)"),
    re.compile(r"complexity (\d+) of"),
    re.compile(r"but got (\d+)"),
    re.compile(r"too many parameters \((\d+)\)"),
    re.compile(r"^(\d+) tokens"),
    re.compile(r"\(complexity: (\d+)\)"),
]


def value_of(msg: str) -> int | None:
    for pattern in _VALUE:
        m = pattern.search(msg)
        if m:
            return int(m.group(1))
    return None


def read_output(path: str, fmt: str) -> str:
    if path == "-":
        text = sys.stdin.read()
    else:
        with open(path, encoding="utf-8-sig", errors="replace") as fh:
            text = fh.read()
    # A clean tsc or ast-grep run prints nothing; every other format writes something.
    if not text.strip() and fmt not in ("tsc", "astgrep"):
        warn("the output is empty: check that the tool ran")
        text = "[]" if fmt in ("eslint", "ruff") else ("{}" if fmt in ("oxlint", "pyright", "golangci", "jscpd", "knip") else "")
    return text


def hits_of(fmt: str, path: str, prefix: str = "", fixes: dict[Site, str] | None = None) -> list[Hit]:
    seen: dict[Site, Hit] = {}
    for rule, f, line, col, msg, *fix in PARSERS[fmt](read_output(path, fmt)):
        h = (rule, f"{prefix}{f}" if prefix and not f.startswith("(") else f, line, col, msg)
        seen.setdefault(h[:4], h)
        if fixes is not None and fix and fix[0]:
            fixes.setdefault(h[:4], fix[0])
    return list(seen.values())


def scope_of(path: str, depth: int) -> str:
    parts = path.split("/")
    return "/".join(parts[:depth]) if len(parts) > depth else "/".join(parts[:-1]) or "."


def print_table(sites: set[Site], depth: int | None, fixes: dict[Site, str] | None = None) -> None:
    by_rule: dict[str, set[Site]] = defaultdict(set)
    for s in sites:
        by_rule[s[0]].add(s)
    rows = sorted(by_rule.items(), key=lambda kv: (-len(kv[1]), kv[0]))
    width = max([len(r) for r, _ in rows] + [4])

    def fix_cols(ss: set[Site]) -> str:
        if not fixes:
            return ""
        kinds = [fixes.get(s, "") for s in ss]
        return f"  {kinds.count('safe'):>4}  {kinds.count('unsafe'):>6}"

    head = f"  {'safe':>4}  {'unsafe':>6}" if fixes else ""
    print(f"{'rule':<{width}}  {'sites':>6}  {'files':>5}{head}")
    for rule, ss in rows:
        print(f"{rule:<{width}}  {len(ss):>6}  {len({s[1] for s in ss}):>5}{fix_cols(ss)}")
        if depth:
            per_scope: dict[str, int] = defaultdict(int)
            for s in ss:
                per_scope[scope_of(s[1], depth)] += 1
            for scope, n in sorted(per_scope.items(), key=lambda kv: (-kv[1], kv[0])):
                print(f"  {scope:<{width - 2}}  {n:>6}")
    print(f"{'total':<{width}}  {len(sites):>6}  {len({s[1] for s in sites}):>5}{fix_cols(sites)}")


def dump(sites: set[Site], path: str) -> None:
    out: dict[str, list[str]] = defaultdict(list)
    for rule, f, line, col in sorted(sites):
        out[rule].append(f"{f}:{line}:{col}")
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(out, fh, indent=1, sort_keys=True)


def load(path: str) -> set[Site]:
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    sites: set[Site] = set()
    for rule, locs in data.items():
        for loc in locs:
            f, line, col = loc.rsplit(":", 2)
            sites.add((rule, f, int(line), int(col)))
    return sites


def ints(text: str) -> list[int]:
    return sorted({int(x) for x in text.split(",") if x.strip()})


def cmd_curve(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="tally.py curve")
    ap.add_argument("format", choices=sorted(PARSERS))
    ap.add_argument("output")
    ap.add_argument("--rule", required=True, help="the rule id as the plain tally prints it")
    ap.add_argument("--at", default="5,8,10,12,15,20,30,50")
    ap.add_argument("--prefix", default="", help="put before every path (jscpd names files from the scanned folder)")
    args = ap.parse_args(argv)
    values: list[tuple[int, str]] = []
    unread = 0
    for rule, f, line, _col, msg in hits_of(args.format, args.output, args.prefix):
        if rule != args.rule:
            continue
        v = value_of(msg)
        if v is None:
            unread += 1
        else:
            values.append((v, f"{f}:{line}"))
    if unread:
        warn(f"{unread} sites of {args.rule} carry no value this script can read")
    if not values:
        warn(f"no site of {args.rule} with a value: check the rule id and that the limit was set low")
        return 1
    print(f"{'limit':>6}  {'sites over it':>13}")
    for t in ints(args.at):
        print(f"{t:>6}  {sum(1 for v, _ in values if v > t):>13}")
    worst = sorted(values, reverse=True)[:5]
    print("highest:", ", ".join(f"{v} {loc}" for v, loc in worst))
    return 0


def tracked(paths: list[str], exts: tuple[str, ...]) -> list[str] | None:
    listed = subprocess.run(["git", "ls-files", "--", *paths], capture_output=True, text=True,
                            encoding="utf-8", check=False)
    if listed.returncode != 0:
        warn(f"git ls-files failed: {listed.stderr.strip()}")
        return None
    return [f for f in listed.stdout.splitlines() if f.endswith(exts)]


def cmd_pattern(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="tally.py pattern")
    ap.add_argument("regex")
    ap.add_argument("paths", nargs="*", default=["."])
    ap.add_argument("--ext", required=True, help="comma-separated, e.g. .rs,.ts")
    ap.add_argument("--expect", type=int)
    ap.add_argument("--scope-depth", type=int)
    args = ap.parse_intermixed_args(argv)  # the paths may follow the options
    files = tracked(args.paths, tuple(e.strip() for e in args.ext.split(",") if e.strip()))
    if files is None:
        return 1
    if not files:
        warn("no tracked file matches the extensions")
        return 1
    pattern = re.compile(args.regex)
    sites: set[Site] = set()
    for f in files:
        with open(f, encoding="utf-8", errors="replace") as fh:
            for n, line in enumerate(fh, 1):
                m = pattern.search(line)
                if m:
                    sites.add((f"pattern:{args.regex}", f, n, m.start() + 1))
    print(f"{len(files)} files read")
    print_table(sites, args.scope_depth)
    if args.expect is not None and len(sites) != args.expect:
        more = len(sites) > args.expect
        print(f"ratchet: {len(sites)} sites, the ratchet holds {args.expect}: "
              + ("new debt; fix it, the count does not go up" if more
                 else f"the count went down; lower the ratchet to {len(sites)} in this change"))
        return 1
    return 0


def cmd_sizes(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="tally.py sizes")
    ap.add_argument("paths", nargs="*", default=["."])
    ap.add_argument("--ext", required=True, help="comma-separated, e.g. .py,.ts")
    ap.add_argument("--lines", default="200,300,400,500,700,1000")
    ap.add_argument("--width", default="80,100,120")
    args = ap.parse_args(argv)
    exts = tuple(e.strip() for e in args.ext.split(",") if e.strip())
    listed = subprocess.run(["git", "ls-files", "--", *args.paths], capture_output=True, text=True,
                            encoding="utf-8", check=False)
    if listed.returncode != 0:
        warn(f"git ls-files failed: {listed.stderr.strip()}")
        return 1
    files = [f for f in listed.stdout.splitlines() if f.endswith(exts)]
    if not files:
        warn("no tracked file matches the extensions")
        return 1
    sizes: list[tuple[int, str]] = []
    widths: list[int] = []
    for f in files:
        with open(f, encoding="utf-8", errors="replace") as fh:
            lines = fh.read().splitlines()
        sizes.append((sum(1 for l in lines if l.strip()), f))
        widths.extend(len(l) for l in lines)
    print(f"{len(files)} files")
    print(f"{'lines':>6}  {'files over it':>13}")
    for t in ints(args.lines):
        print(f"{t:>6}  {sum(1 for n, _ in sizes if n > t):>13}")
    print("largest:", ", ".join(f"{n} {f}" for n, f in sorted(sizes, reverse=True)[:5]))
    print(f"{'width':>6}  {'lines over it':>13}")
    for t in ints(args.width):
        print(f"{t:>6}  {sum(1 for w in widths if w > t):>13}")
    return 0


_HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def added_lines(base: str) -> dict[str, set[int]]:
    """Path -> the line numbers the working tree adds against BASE."""
    out = subprocess.run(["git", "diff", "-U0", "--no-color", "--no-ext-diff", base, "--"],
                         capture_output=True, text=True, encoding="utf-8", errors="replace")
    if out.returncode != 0:
        sys.exit(f"git diff {base} failed: {out.stderr.strip()}")
    lines: dict[str, set[int]] = defaultdict(set)
    path = None
    for line in out.stdout.splitlines():
        if line.startswith("+++ "):
            path = None if line[4:] == "/dev/null" else line[4:].removeprefix("b/")
        elif path and (m := _HUNK.match(line)):
            start, count = int(m.group(1)), int(m.group(2) or 1)
            lines[path].update(range(start, start + count))
    # Files git does not track yet are new code too.
    untracked = subprocess.run(["git", "ls-files", "--others", "--exclude-standard"],
                               capture_output=True, text=True).stdout.split()
    for f in untracked:
        try:
            with open(f, encoding="utf-8", errors="replace") as fh:
                lines[f].update(range(1, sum(1 for _ in fh) + 1))
        except OSError:
            continue
    return lines


def cmd_added(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="tally.py added")
    ap.add_argument("format", choices=sorted(PARSERS))
    ap.add_argument("output")
    ap.add_argument("--base", required=True, help="the merge base the change is measured against")
    ap.add_argument("--ignore", default="", help="rules left out, comma-separated")
    args = ap.parse_args(argv)
    skip = {r for r in args.ignore.split(",") if r}
    new = added_lines(args.base)
    hits = sorted((f, line, col, rule, msg) for rule, f, line, col, msg in hits_of(args.format, args.output)
                  if rule not in skip and line in new.get(f, ()))
    for f, line, col, rule, msg in hits:
        print(f"{f}:{line}:{col}: {rule} {msg}")
    print(f"{len(hits)} sites on lines this change adds"
          + ("; new code meets the whole target, debt rules too: fix them" if hits else ""))
    return 1 if hits else 0


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "added":
        return cmd_added(sys.argv[2:])
    if len(sys.argv) > 1 and sys.argv[1] == "curve":
        return cmd_curve(sys.argv[2:])
    if len(sys.argv) > 1 and sys.argv[1] == "pattern":
        return cmd_pattern(sys.argv[2:])
    if len(sys.argv) > 1 and sys.argv[1] == "sizes":
        return cmd_sizes(sys.argv[2:])
    if len(sys.argv) > 1 and sys.argv[1] == "diff":
        ap = argparse.ArgumentParser(prog="tally.py diff")
        ap.add_argument("a")
        ap.add_argument("b")
        ap.add_argument("--json")
        ap.add_argument("--scope-depth", type=int)
        args = ap.parse_args(sys.argv[2:])
        sites = load(args.b) - load(args.a)
        fixes: dict[Site, str] = {}
    else:
        ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
        ap.add_argument("format", choices=sorted(PARSERS))
        ap.add_argument("output", help="the tool's output file, or - for stdin")
        ap.add_argument("--json", help="write the sites to this file, for `diff`")
        ap.add_argument("--scope-depth", type=int)
        ap.add_argument("--prefix", default="", help="put before every path (jscpd names files from the scanned folder)")
        ap.add_argument("--expect", type=int, help="a ratchet: fail unless the total is exactly this")
        args = ap.parse_args()
        fixes = {}
        sites = {h[:4] for h in hits_of(args.format, args.output, args.prefix, fixes)}
    if args.json:
        dump(sites, args.json)
    print_table(sites, args.scope_depth, fixes)
    expect = getattr(args, "expect", None)
    if expect is not None and len(sites) != expect:
        more = len(sites) > expect
        print(f"ratchet: {len(sites)} sites, the ratchet holds {expect}: "
              + ("new debt; fix it, the count does not go up" if more
                 else f"the count went down; lower the ratchet to {len(sites)} in this change"))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
