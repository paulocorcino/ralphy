#!/usr/bin/env python3
"""Compare a tool's own list of rules with a reference and with a draft config.

The references of this skill are a cache with an `as of` version; the tool is
the source of truth. Each tool prints its rules:

  oxlint    oxlint --rules --format json > LIST
  ruff      ruff rule --all --output-format json > LIST
  clippy    cargo clippy -q -- -W help > LIST           (rustc and clippy lints)
  tsc       tsc --all > LIST                            (compiler options)
  golangci  golangci-lint linters > LIST                (with every linter enabled)

  drift.py diff FORMAT OLD_LIST NEW_LIST    # rules added and removed between two versions
  drift.py check FORMAT LIST DRAFT          # names in the draft the tool does not know,
                                            # or marks deprecated, removed or preview

  drift.py age [--days N] REF...           # each `As of` line against the registries

`diff` takes the list printed by the reference's `as of` version and the list of
the pinned version: the added rules are the candidates nobody has looked at yet.
`age` reads `As of YYYY-MM-DD: tool 1.2.3, ...` and asks npm, PyPI or GitHub for
each tool's latest stable release. It exits 1, research required, when the date
is older than N days (default 90), a tool has a newer major (a newer minor for a
0.x tool), or a registry did not answer.
`check` reads the draft config (oxlintrc JSON with comments, ruff TOML or
pyproject, Cargo.toml lints, tsconfig JSON with comments, .golangci.yml) and
needs no second version.
"""

from __future__ import annotations

import datetime
import json
import re
import sys
import tomllib
import urllib.request
from pathlib import Path

Rules = dict[str, str]  # name -> status ("stable", "deprecated", "preview", ...)


def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8-sig", errors="replace")


def jsonc(text: str) -> object:
    text = re.sub(r'("(?:\\.|[^"\\])*")|//[^\n]*|/\*.*?\*/', lambda m: m.group(1) or "", text, flags=re.S)
    text = re.sub(r",(\s*[}\]])", r"\1", text)
    return json.loads(text)


# ---- the tool's list -------------------------------------------------------

def list_oxlint(text: str) -> Rules:
    return {f"{r['scope']}/{r['value']}": "stable" for r in json.loads(text)}


def list_ruff(text: str) -> Rules:
    out = {}
    for r in json.loads(text):
        if not r.get("code"):  # a rule listed without a code cannot be named in a config
            continue
        status = r.get("status")
        out[r["code"]] = (next(iter(status)) if isinstance(status, dict) else str(status or "stable")).lower()
    return out


_LINT_ROW = re.compile(r"^\s*((?:clippy::)?[a-z][\w-]*)\s+(allow|warn|deny|forbid)\s")
_GROUP_ROW = re.compile(r"^\s*((?:clippy::)?[a-z][\w-]*)\s+((?:clippy::)?[a-z][\w-]*(?:,\s*)?)+\s*$")


def list_clippy(text: str) -> Rules:
    out = {}
    for line in text.splitlines():
        m = _LINT_ROW.match(line) or _GROUP_ROW.match(line)
        if m and m.group(1) not in ("name", "----"):
            out[m.group(1).replace("-", "_")] = "stable"
    return out


def list_tsc(text: str) -> Rules:
    out = {}
    for line in text.splitlines():
        if line.startswith("--"):
            out[line[2:].split(",")[0].strip()] = "stable"
    return out


def list_golangci(text: str) -> Rules:
    out = {}
    for line in text.splitlines():
        m = re.match(r"^([a-z0-9_]+)( \[deprecated\])?: (.*)$", line)
        if m:
            out[m.group(1)] = "deprecated" if m.group(2) or "deprecated" in m.group(3).lower() else "stable"
    return out


LISTS = {"oxlint": list_oxlint, "ruff": list_ruff, "clippy": list_clippy, "tsc": list_tsc,
         "golangci": list_golangci}


# ---- the names a draft uses ------------------------------------------------

def names_oxlint(text: str) -> list[str]:
    cfg = jsonc(text)
    keys = list((cfg.get("rules") or {}).keys())
    for o in cfg.get("overrides") or []:
        keys += list((o.get("rules") or {}).keys())
    return [k if "/" in k else f"eslint/{k}" for k in keys]


def names_ruff(text: str) -> list[str]:
    cfg = tomllib.loads(text)
    lint = cfg.get("tool", {}).get("ruff", cfg).get("lint", cfg.get("tool", {}).get("ruff", cfg))
    codes: list[str] = []
    for key in ("select", "extend-select", "ignore", "extend-ignore", "fixable", "unfixable"):
        codes += lint.get(key, [])
    for key in ("per-file-ignores", "extend-per-file-ignores"):
        for v in (lint.get(key) or {}).values():
            codes += v
    return [c for c in codes if c != "ALL"]


def names_clippy(text: str) -> list[str]:
    cfg = tomllib.loads(text)
    tables = [cfg.get("workspace", {}).get("lints", {}), cfg.get("lints", {})]
    out = []
    for t in tables:
        out += [f"clippy::{k}" for k in (t.get("clippy") or {})]
        out += list(t.get("rust") or {})
    return out


def names_tsc(text: str) -> list[str]:
    return list((jsonc(text).get("compilerOptions") or {}).keys())


def disabled_golangci(text: str) -> set[str]:
    """The names under a `disable:` list: a deprecated linter belongs there, since
    `default: all` turns deprecated linters on too."""
    out, section = set(), None
    for line in text.splitlines():
        m = re.match(r"^\s{2,4}(enable|disable):\s*$", line)
        if m:
            section = m.group(1)
            continue
        if section and re.match(r"^\s{2,4}\S", line) and not line.strip().startswith(("-", "#")):
            section = None
        m = re.match(r"^\s+-\s+([a-z0-9_]+)\s*(#.*)?$", line)
        if section == "disable" and m:
            out.add(m.group(1))
    return out


def names_golangci(text: str) -> list[str]:
    out, section = [], None
    for line in text.splitlines():
        m = re.match(r"^\s{2,4}(enable|disable):\s*$", line)
        if m:
            section = m.group(1)
            continue
        if section and re.match(r"^\s{2,4}\S", line) and not line.strip().startswith("-"):
            section = None
        m = re.match(r"^\s+-\s+([a-z0-9_]+)\s*(#.*)?$", line)
        if section and m:
            out.append(m.group(1))
    return out


NAMES = {"oxlint": names_oxlint, "ruff": names_ruff, "clippy": names_clippy, "tsc": names_tsc,
         "golangci": names_golangci}


def known(fmt: str, name: str, rules: Rules) -> str | None:
    """The status of a name, or None when the tool does not know it."""
    if name in rules:
        return rules[name]
    if fmt == "ruff":  # a prefix selects a family: "D", "PLR09", "TRY"
        hits = [s for code, s in rules.items() if code.startswith(name)]
        return "stable" if hits else None
    if fmt == "tsc":
        lower = {k.lower(): v for k, v in rules.items()}
        return lower.get(name.lower())
    return None


# ---- the age of a reference ------------------------------------------------

REGISTRY = {  # tool name in an `As of` line -> where its latest stable release is
    "oxlint": "npm:oxlint", "oxlint-tsgolint": "npm:oxlint-tsgolint", "typescript": "npm:typescript",
    "typescript-eslint": "npm:typescript-eslint", "eslint": "npm:eslint", "knip": "npm:knip",
    "jscpd": "npm:jscpd", "ruff": "pypi:ruff", "mypy": "pypi:mypy", "pyright": "pypi:pyright",
    "vulture": "pypi:vulture", "ast-grep": "npm:@ast-grep/cli", "deptry": "pypi:deptry",
    "pip-audit": "pypi:pip-audit", "complexipy": "pypi:complexipy", "golangci-lint": "github:golangci/golangci-lint",
    "Rust": "github:rust-lang/rust", "clippy": None,  # clippy ships with Rust
}
_ASOF = re.compile(r"^As of (\d{4}-\d{2}-\d{2}): (.*?\.)(?:\s|$)", re.M)
_PAIR = re.compile(r"([A-Za-z][\w.-]*) v?(\d+(?:\.\d+)*)\b")


def latest(source: str) -> str:
    kind, name = source.split(":", 1)
    url = {"npm": f"https://registry.npmjs.org/{name}/latest", "pypi": f"https://pypi.org/pypi/{name}/json",
           "github": f"https://api.github.com/repos/{name}/releases/latest"}[kind]
    req = urllib.request.Request(url, headers={"User-Agent": "lint-gate-drift"})
    with urllib.request.urlopen(req, timeout=20) as r:
        data = json.load(r)
    return {"npm": lambda: data["version"], "pypi": lambda: data["info"]["version"],
            "github": lambda: data["tag_name"]}[kind]().lstrip("v")


def ver(s: str) -> tuple[int, ...]:
    return tuple(int(x) for x in re.findall(r"\d+", s)[:3])


def age(refs: list[str], days: int) -> int:
    today, research = datetime.date.today(), False
    for ref in refs:
        m = _ASOF.search(read(ref))
        if not m:
            print(f"{ref}: no `As of YYYY-MM-DD:` line: research required")
            research = True
            continue
        reasons = []
        old = (today - datetime.date.fromisoformat(m.group(1))).days
        if old > days:
            reasons.append(f"{old} days old")
        print(f"{ref}: as of {m.group(1)} ({old} days)")
        for tool, have in _PAIR.findall(m.group(2)):
            source = REGISTRY.get(tool, "?")
            if source is None:
                continue
            if source == "?":
                print(f"  {tool} {have}: no registry known, check by hand")
                reasons.append(f"{tool} unchecked")
                continue
            try:
                new = latest(source)
            except Exception as e:  # noqa: BLE001 - a registry that does not answer is a reason to research
                print(f"  {tool} {have}: registry did not answer ({e})")
                reasons.append(f"{tool} unchecked")
                continue
            a = ver(have)
            b = ver(new)[: len(a)]  # "typescript-eslint 8" names a major: any 8.x is current
            level = 0 if a and a[0] else 1  # the minor is the major of a 0.x tool
            if b[: level + 1] > a[: level + 1]:
                state = "newer major: research"
                reasons.append(f"{tool} {have} -> {new}")
            elif b > a:
                state = "newer: drift.py diff"
            else:
                state = "current"
            print(f"  {tool} {have} -> {new}: {state}")
        print(f"  research: {'required (' + '; '.join(reasons) + ')' if reasons else 'not required'}")
        research = research or bool(reasons)
    return 1 if research else 0


def main() -> int:
    if len(sys.argv) >= 3 and sys.argv[1] == "age":
        args, days = sys.argv[2:], 90
        if args[0] == "--days" and len(args) >= 3:
            days, args = int(args[1]), args[2:]
        return age(args, days)
    if len(sys.argv) != 5 or sys.argv[1] not in ("diff", "check") or sys.argv[2] not in LISTS:
        print(__doc__, file=sys.stderr)
        return 2
    op, fmt, a, b = sys.argv[1:]
    if op == "diff":
        old, new = LISTS[fmt](read(a)), LISTS[fmt](read(b))
        if not old or not new:
            print("warning: an empty list: check the command that printed it", file=sys.stderr)
        added, removed = sorted(set(new) - set(old)), sorted(set(old) - set(new))
        changed = sorted(n for n in set(old) & set(new) if old[n] != new[n])
        print(f"{len(old)} -> {len(new)} rules")
        print(f"added ({len(added)}):", *added, sep="\n  ")
        print(f"removed ({len(removed)}):", *removed, sep="\n  ")
        print(f"status changed ({len(changed)}):", *[f"{n}: {old[n]} -> {new[n]}" for n in changed], sep="\n  ")
        return 0
    rules = LISTS[fmt](read(a))
    if not rules:
        print("warning: the tool's list is empty: check the command that printed it", file=sys.stderr)
        return 1
    bad = 0
    draft = read(b)
    disabled = disabled_golangci(draft) if fmt == "golangci" else set()
    for name in NAMES[fmt](draft):
        status = known(fmt, name, rules)
        if status == "deprecated" and name in disabled:
            continue  # off on purpose: `default: all` would turn it on
        if status is None:
            print(f"unknown: {name} (renamed or removed: find it in the changelog)")
            bad += 1
        elif status not in ("stable", "allow", "warn", "deny"):
            print(f"{status}: {name}")
            bad += 1
    print(f"{bad} names to resolve" if bad else "every name in the draft is known and stable")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
