#!/usr/bin/env python3
"""Write the debt list of a lint group: the block that turns a group on and keeps
each rule with sites off, marked `debt` with its count, or `noise` with its reason.

The block is the step-5 flip for a group. It is printed, not written: paste it in
the config where the comment says, keeping the file's line endings.

  debt.py oxlint --rules LIST --config CONFIG --sites SITES... [--plugins p,..]
                 [--categories c,..] [--target r,..] [--noise NOISE.json] [--plan PATH]
  debt.py clippy --help-list HELP --sites SITES... --groups g,.. [--target l,..]
                 [--noise NOISE.json] [--plan PATH]
  debt.py golangci --sites SITES... [--noise NOISE.json] [--plan PATH] [--by-id gosec,gocritic]
  debt.py ruff --sites SITES... [--noise NOISE.json] [--plan PATH]
  debt.py mypy --sites SITES... [--noise NOISE.json] [--plan PATH]
  debt.py shrink BASE CONFIG [--plan PLAN]   # fails when the debt list of CONFIG grew against BASE

`shrink` is the check that keeps the list going one way: an entry that BASE does
not have, or a count above its BASE count, fails it. CI runs it against the base
branch: `git show origin/main:Cargo.toml > base.toml && debt.py shrink base.toml Cargo.toml`.
A base with no debt list (or no file) passes: that change brings the first list.
`shrink` also fails on an entry added to a debt list with no `debt:` or
`noise:` comment (a rule turned off with no reason, which no count sees), when
a `debt:` comment is tied to no entry (a config form it
does not read would pass any growth), and on a new `noise:` entry whose reason
cites no decision id (`D3`, `ADR-12`) found in PLAN: a rule off for good is a
decision, not a way out of the debt list.

  LIST     oxlint --rules --format json, of the pinned version
  CONFIG   the repo's .oxlintrc.json today (its plugins, categories and rules)
  HELP     cargo clippy -q -- -W help, of the pinned toolchain
  SITES    tally.py --json files of the draft runs (production and every target);
           a site in any of them counts once
  NOISE    {"<rule>": "<reason>"}: the rules off for good
  --target the rules the target names one by one, on top of the groups

oxlint: the rules that come on are those of every category and plugin on after
the change, minus those on before, plus --target. A new plugin brings its rules
of the categories already on, so they are counted too. A rule the config already
sets keeps its setting. A rule with sites is `off` under `// debt: N sites`, a
noise rule under `// noise: <reason>`, a rule with no site is on by its category
or, for --target, by an `"error"` line.

clippy: each group of --groups goes to `deny` with priority -1; each lint of the
group with sites is `allow` under `# debt: N sites`. A --target lint with no site
is `deny`; one with sites is not in the block: the plan turns it on.

golangci: `default: all`, and each linter with sites goes in `disable` under
`# debt: N sites` (a linter's sites are the sum of its check ids: `gosec:G104`,
`revive:argument-limit`); a noise linter under `# noise: <reason>`. Merge the
target's own `disable` lines (the deprecated names `default: all` would turn on)
into the block. `--by-id` keeps a linter on and lists only its check ids with
sites, under `settings: <linter>:` (`excludes` for gosec, `disabled-checks` for
gocritic): its other checks stay enforced while the debt is paid.

Every comment sits on its own line: some config readers strip only whole-line
comments.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from drift import jsonc  # noqa: E402  (a sibling script, not a package)


def site_counts(paths: list[str]) -> dict[str, int]:
    """Rule -> sites, the union of the tally files, with oxlint codes as `scope/rule`."""
    union: dict[str, set[str]] = {}
    for p in paths:
        for rule, locs in json.loads(Path(p).read_text(encoding="utf-8")).items():
            m = re.fullmatch(r"([\w-]+)\((.+)\)", rule)
            name = f"{m.group(1)}/{m.group(2)}" if m else rule
            union.setdefault(name, set()).update(locs)
    return {k: len(v) for k, v in union.items()}


def plural(n: int) -> str:
    return f"{n} site" if n == 1 else f"{n} sites"


def oxlint(args: argparse.Namespace) -> int:
    listing = json.loads(Path(args.rules).read_text(encoding="utf-8"))
    cfg = jsonc(Path(args.config).read_text(encoding="utf-8-sig"))
    sites = site_counts(args.sites)
    noise = json.loads(Path(args.noise).read_text(encoding="utf-8")) if args.noise else {}
    plugins_before = set(cfg.get("plugins") or [])
    cats_before = {c for c, lvl in (cfg.get("categories") or {}).items() if lvl in ("error", "warn")}
    plugins_after = plugins_before | set(filter(None, args.plugins.split(",")))
    cats_after = cats_before | set(filter(None, args.categories.split(",")))
    target = set(filter(None, args.target.split(",")))

    def full(name: str) -> str:
        return name if "/" in name else f"eslint/{name}"

    def key(name: str) -> str:
        return name.split("/", 1)[1] if name.startswith("eslint/") else name

    configured = {full(k) for k in (cfg.get("rules") or {})}
    active_before, active_after = set(), set()
    for r in listing:
        name = f"{r['scope']}/{r['value']}"
        plugin_on = r["scope"] in plugins_before or r["scope"] == "eslint"
        if plugin_on and r["category"] in cats_before:
            active_before.add(name)
        if (r["scope"] in plugins_after or r["scope"] == "eslint") and r["category"] in cats_after:
            active_after.add(name)
    known = {f"{r['scope']}/{r['value']}" for r in listing}
    unknown = sorted(t for t in map(full, target) if t not in known)
    if unknown:
        print("unknown --target rules:", ", ".join(unknown), file=sys.stderr)
        return 1
    coming = (active_after - active_before) | {full(t) for t in target}
    on, debt, off = [], [], []
    for name in sorted(coming - configured):
        if name in noise or key(name) in noise:
            off.append(name)
        elif sites.get(name, 0):
            debt.append(name)
        elif name in {full(t) for t in target} and name not in active_after:
            on.append(name)
    plan = args.plan or "the transition plan"
    out = [
        f'"plugins": {json.dumps(sorted(plugins_after))},',
        f'"categories": {json.dumps({c: "error" for c in sorted(cats_after)})},',
        "",
        "// The lint gate. A rule `off` below is one of two kinds. `debt`: it has",
        f"// sites today and {plan} turns it on; this list only shrinks.",
        "// `noise`: off for good, with the reason.",
    ]
    out += [f'"{key(n)}": "error",' for n in on]
    for n in sorted(debt, key=lambda n: (-sites[n], n)):
        out += [f"// debt: {plural(sites[n])}", f'"{key(n)}": "off",']
    for n in off:
        out += [f"// noise: {noise.get(n) or noise.get(key(n))}", f'"{key(n)}": "off",']
    print("\n".join(out))
    newly = len(coming - configured)
    print(f"\n{newly} rules come on: {newly - len(debt) - len(off)} with no site, "
          f"{len(debt)} debt ({sum(sites[n] for n in debt)} sites), {len(off)} noise", file=sys.stderr)
    return 0


def clippy(args: argparse.Namespace) -> int:
    text = Path(args.help_list).read_text(encoding="utf-8", errors="replace")
    sites = site_counts(args.sites)
    noise = json.loads(Path(args.noise).read_text(encoding="utf-8")) if args.noise else {}
    plan = args.plan or "the transition plan"
    rust, clip = [], []
    total_debt = 0
    for group in filter(None, args.groups.split(",")):
        g = group.replace("clippy::", "")
        m = re.search(rf"^\s*(?:clippy::)?{re.escape(g.replace('_', '-'))}\s+(.+)$", text, re.M)
        if not m:
            print(f"no group {group} in the help list", file=sys.stderr)
            return 1
        is_clippy = group.startswith("clippy::") or "clippy::" in m.group(1)
        members = [x.strip().replace("clippy::", "").replace("-", "_") for x in m.group(1).split(",")]
        lines = rust if not is_clippy else clip
        lines += ["", f"# The lint gate: `{g}` is on. A lint `allow` below is debt that {plan}",
                  "# turns on (this list only shrinks), or noise, off for good with its reason.",
                  f'{g} = {{ level = "deny", priority = -1 }}']
        for lint in sorted(members):
            full = f"clippy::{lint}" if is_clippy else lint
            if lint in noise or full in noise:
                lines += [f"# noise: {noise.get(lint) or noise.get(full)}", f'{lint} = "allow"']
        debts = [(sites[f"clippy::{l}" if is_clippy else l], l) for l in members
                 if sites.get(f"clippy::{l}" if is_clippy else l, 0) and l not in noise
                 and f"clippy::{l}" not in noise]
        for n, lint in sorted(debts, key=lambda x: (-x[0], x[1])):
            lines += [f"# debt: {plural(n)}", f'{lint} = "allow"']
        total_debt += sum(n for n, _ in debts)
    for t in filter(None, args.target.split(",")):
        lint = t.replace("clippy::", "")
        is_clippy = t.startswith("clippy::")
        if not sites.get(t, 0):
            (clip if is_clippy else rust).append(f'{lint} = "deny"')
    print("[workspace.lints.rust]" + "\n".join([""] + rust))
    print("\n[workspace.lints.clippy]" + "\n".join([""] + clip))
    print(f"\ndebt in the groups: {total_debt} sites", file=sys.stderr)
    return 0


def _ranked(paths: list[str], skip: set[str]) -> list[tuple[int, str]]:
    sites: dict[str, set[str]] = {}
    for p in paths:
        for rule, locs in json.loads(Path(p).read_text(encoding="utf-8")).items():
            sites.setdefault(rule.split(":")[-1], set()).update(locs)
    return sorted(((len(v), k) for k, v in sites.items() if v and k not in skip), key=lambda x: (-x[0], x[1]))


def ruff(args: argparse.Namespace) -> int:
    noise = json.loads(Path(args.noise).read_text(encoding="utf-8")) if args.noise else {}
    plan = args.plan or "the transition plan"
    debts = _ranked(args.sites, set(noise))
    out = ['select = ["ALL"]',
           f"# The lint gate: every rule is on. A code in `ignore` is debt that {plan}",
           "# turns on (this list only shrinks), or noise, off for good with its reason.",
           "ignore = ["]
    for code in sorted(noise):
        out += [f"  # noise: {noise[code]}", f'  "{code}",']
    for n, code in debts:
        out += [f"  # debt: {plural(n)}", f'  "{code}",']
    out.append("]")
    print("\n".join(out))
    print(f"\n{len(debts)} codes in debt ({sum(n for n, _ in debts)} sites), {len(noise)} noise", file=sys.stderr)
    return 0


def mypy(args: argparse.Namespace) -> int:
    noise = json.loads(Path(args.noise).read_text(encoding="utf-8")) if args.noise else {}
    plan = args.plan or "the transition plan"
    enabled = {x.strip() for x in (args.enabled or "").split(",") if x.strip()}
    debts = _ranked(args.sites, set(noise))
    out = [f"# The type gate: `strict`. A code in `disable_error_code` is debt that {plan}",
           "# turns on (this list only shrinks), or noise, off for good with its reason.",
           "disable_error_code ="]
    for code in sorted(noise):
        out += [f"    # noise: {noise[code]}", f"    {code},"]
    for n, code in debts:
        out += [f"    # debt: {plural(n)}", f"    {code},"]
    print("\n".join(out))
    clash = sorted(enabled & {c for _, c in debts})
    if clash:
        print("\nremove from enable_error_code while they have debt (mypy does not disable a code "
              f"that enable_error_code names): {', '.join(clash)}", file=sys.stderr)
    print(f"{len(debts)} codes in debt ({sum(n for n, _ in debts)} sites), {len(noise)} noise", file=sys.stderr)
    return 0


_BY_ID = {"gosec": "excludes", "gocritic": "disabled-checks"}


def golangci(args: argparse.Namespace) -> int:
    by_id = {x for x in (args.by_id or "").split(",") if x}
    unknown = by_id - set(_BY_ID)
    if unknown:
        print(f"--by-id supports {', '.join(_BY_ID)}, not {', '.join(sorted(unknown))}", file=sys.stderr)
        return 2
    per_linter: dict[str, set[str]] = {}
    per_id: dict[str, dict[str, set[str]]] = {}
    for p in args.sites:
        for rule, locs in json.loads(Path(p).read_text(encoding="utf-8")).items():
            linter, _, check = rule.partition(":")
            if linter in by_id and check:
                per_id.setdefault(linter, {}).setdefault(check, set()).update(locs)
            else:
                per_linter.setdefault(linter, set()).update(locs)
    noise = json.loads(Path(args.noise).read_text(encoding="utf-8")) if args.noise else {}
    plan = args.plan or "the transition plan"
    out = ["linters:", "  default: all",
           f"  # The lint gate: every linter is on. A linter in `disable` is debt that {plan}",
           "  # turns on (this list only shrinks), or noise, off for good with its reason.",
           "  disable:"]
    for name in sorted(noise):
        out += [f"    # noise: {noise[name]}", f"    - {name}"]
    debts = sorted(((len(v), k) for k, v in per_linter.items() if v and k not in noise), key=lambda x: (-x[0], x[1]))
    for n, name in debts:
        out += [f"    # debt: {plural(n)}", f"    - {name}"]
    if per_id:
        out += ["  settings:"]
        for linter in sorted(per_id):
            out += [f"    {linter}:", f"      {_BY_ID[linter]}:"]
            ids = sorted(((len(v), k) for k, v in per_id[linter].items()), key=lambda x: (-x[0], x[1]))
            out += [line for n, k in ids for line in (f"        # debt: {plural(n)}", f"        - {k}")]
    print("\n".join(out))
    for linter in sorted(per_id):
        print(f"{linter}: on, {len(per_id[linter])} check ids in debt "
              f"({sum(len(v) for v in per_id[linter].values())} sites)", file=sys.stderr)
    print(f"{len(debts)} linters in debt ({sum(n for n, _ in debts)} sites), {len(noise)} noise", file=sys.stderr)
    return 0


# One entry per line, in any of the config forms a debt list takes:
#   "rule": ...  /  lint = ...        a key (JSON, TOML table)
#   - linter                          a YAML list item
#   "CODE",                           a TOML array item (ruff `ignore`)
#   error-code,                       an INI list item (mypy `disable_error_code`)
_KEY = (r'^\s*(?:-\s+"?(?P<yaml>[\w/@.-]+)"?(?=\s*(?:#|$))'
        r'|"(?P<item>[\w/@.-]+)",?(?=\s*(?:#|//|$))'
        r'|"?(?P<key>[\w/@.-]+)"?\s*[:=]'
        r'|(?P<bare>[A-Za-z][\w-]*),?(?=\s*(?:#|$)))')
_ITEMS = re.compile(r'^\s*(?:"[\w/@.-]+",?\s*)+(?:#.*)?$')
_COUNT = r'(?:#|//)\s*debt:\s*(\d+) sites?'
_NOISE = r'(?:#|//)\s*noise:\s*(.+?)\s*$'
_DECISION = re.compile(r'\b(D\d+|ADR-\d+)\b')


def _name(m: re.Match) -> str:
    return m.group("yaml") or m.group("item") or m.group("key") or m.group("bare")


_OFF = re.compile(r'[:=]\s*"?(off|allow)"?\s*,?\s*(?:#.*|//.*)?$')


def entries(text: str) -> tuple[dict[str, int], dict[str, str], int, set[str]]:
    """The debt and noise entries of a config, the count of `debt:` comments
    tied to no entry, and the plain entries: a list item (`"X",`, `- x`, `x,`)
    or a key set `off`/`allow` with no `debt:` or `noise:` comment, in a block
    (a run of entry, comment and blank lines) that holds a debt or noise
    entry, so a list of paths or members stays out. A comment
    sits on its own line above its entry, or at the end of the entry's line;
    a noise comment may cover a line of several items."""
    debt: dict[str, int] = {}
    noise: dict[str, str] = {}
    plain: set[str] = set()
    block: set[str] = set()
    listed = False
    comments = 0
    pending_debt: int | None = None
    pending_noise: str | None = None
    for line in text.replace("\r\n", "\n").split("\n"):
        before = len(debt) + len(noise)
        inline_debt = re.match(_KEY + r'.*?' + _COUNT, line)
        inline_noise = re.match(_KEY + r'.*?' + _NOISE, line)
        alone_debt = re.match(r'^\s*' + _COUNT + r'\b.*$', line)  # a note may follow: `; Phase 1`
        alone_noise = re.match(r'^\s*' + _NOISE, line)
        key = re.match(_KEY, line)
        if re.search(r'(?:#|//)\s*debt:', line) and not (inline_debt or alone_debt):
            comments += 1  # a `debt:` comment in a form this reads not: counted, so it shows as tied to no entry
        if inline_debt:
            comments += 1
            debt[_name(inline_debt)] = int(inline_debt.group(len(inline_debt.groups())))
            pending_debt = pending_noise = None
        elif inline_noise:
            noise[_name(inline_noise)] = inline_noise.group(len(inline_noise.groups()))
            pending_debt = pending_noise = None
        elif alone_debt:
            comments += 1
            pending_debt, pending_noise = int(alone_debt.group(1)), None
        elif alone_noise:
            pending_noise, pending_debt = alone_noise.group(1), None
        elif pending_noise is not None and _ITEMS.match(line):
            for name in re.findall(r'"([\w/@.-]+)"', line):
                noise[name] = pending_noise
            pending_noise = None
        elif key and pending_debt is not None:
            debt[_name(key)] = pending_debt
            pending_debt = None
        elif key and pending_noise is not None:
            noise[_name(key)] = pending_noise
            pending_noise = None
        elif key and (key.group("item") or key.group("yaml") or key.group("bare") or _OFF.search(line)):
            block.add(_name(key))
            pending_debt = pending_noise = None
        elif line.strip() and not line.strip().startswith(("#", "//")) and not key:
            # A line that is no entry and no comment ends the block.
            plain |= block if listed else set()
            block, listed = set(), False
            pending_debt = pending_noise = None
        elif line.strip() and not line.strip().startswith(("#", "//")):
            pending_debt = pending_noise = None
        listed = listed or len(debt) + len(noise) > before
    plain |= block if listed else set()
    return debt, noise, comments - len(debt), plain


def debt_entries(text: str) -> dict[str, int]:
    return entries(text)[0]


def shrink(args: argparse.Namespace) -> int:
    base_path = Path(args.base)
    base, base_noise, _, base_plain = (entries(base_path.read_text(encoding="utf-8-sig"))
                                       if base_path.exists() else ({}, {}, 0, set()))
    head, head_noise, unread, head_plain = entries(Path(args.config).read_text(encoding="utf-8-sig"))
    if unread:
        # A debt comment no entry follows: a form this script does not read, which would pass any growth.
        print(f"{unread} `debt:` comments are tied to no entry: shrink cannot read this config's form")
        return 1
    plan = Path(args.plan).read_text(encoding="utf-8-sig") if args.plan else ""
    bad = []
    for name, reason in head_noise.items():
        if name in base_noise or not base:
            continue
        cited = _DECISION.findall(reason)
        if not cited or not all(re.search(rf"\b{re.escape(c)}\b", plan) for c in cited):
            bad.append(f"new noise entry: {name} ({reason}): a rule off for good is a decision; "
                       "cite its id (Dn, ADR-n) in the reason, recorded in the plan (--plan)")
    if not base:
        # The change that brings the first debt list has nothing to shrink against.
        print(f"the base has no debt list: this change brings the first one, "
              f"{len(head)} entries ({sum(head.values())} sites), {len(head_noise)} noise")
        return 0
    bad += [f"new debt entry: {r} ({n})" for r, n in head.items() if r not in base]
    # A rule turned off with no comment is an exit the counts above never see.
    bad += [f"new entry with no `debt:` or `noise:` comment: {r}: a rule off says why"
            for r in sorted(head_plain - base_plain)]
    bad += [f"debt grew: {r} {base[r]} -> {n}" for r, n in head.items() if r in base and n > base[r]]
    for line in bad:
        print(line)
    gone = sorted(set(base) - set(head))
    print(f"{len(head)} debt entries ({sum(head.values())} sites), {len(gone)} gone since the base"
          + ("" if bad else "; the list did not grow"))
    return 1 if bad else 0


def main() -> int:
    if len(sys.argv) >= 2 and sys.argv[1] == "shrink":
        rest = sys.argv[2:]
        plan = None
        if "--plan" in rest:
            i = rest.index("--plan")
            plan, rest = (rest[i + 1] if i + 1 < len(rest) else None), rest[:i] + rest[i + 2:]
        if len(rest) != 2 or (sys.argv[2:3] and sys.argv[2] in ("-h", "--help")):
            print(__doc__)
            return 0 if sys.argv[2:3] in (["-h"], ["--help"]) else 2
        return shrink(argparse.Namespace(base=rest[0], config=rest[1], plan=plan))
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="tool", required=True)
    o = sub.add_parser("oxlint")
    o.add_argument("--rules", required=True)
    o.add_argument("--config", required=True)
    o.add_argument("--sites", nargs="+", required=True)
    o.add_argument("--plugins", default="")
    o.add_argument("--categories", default="")
    o.add_argument("--target", default="")
    o.add_argument("--noise")
    o.add_argument("--plan")
    g = sub.add_parser("golangci")
    g.add_argument("--sites", nargs="+", required=True)
    g.add_argument("--noise")
    g.add_argument("--plan")
    g.add_argument("--by-id")
    for name in ("ruff", "mypy"):
        s = sub.add_parser(name)
        s.add_argument("--sites", nargs="+", required=True)
        s.add_argument("--noise")
        s.add_argument("--plan")
        if name == "mypy":
            s.add_argument("--enabled", help="the codes of enable_error_code, comma-separated")
    c = sub.add_parser("clippy")
    c.add_argument("--help-list", required=True)
    c.add_argument("--sites", nargs="+", required=True)
    c.add_argument("--groups", required=True)
    c.add_argument("--target", default="")
    c.add_argument("--noise")
    c.add_argument("--plan")
    args = ap.parse_args()
    return {"oxlint": oxlint, "clippy": clippy, "golangci": golangci, "ruff": ruff, "mypy": mypy}[args.tool](args)


if __name__ == "__main__":
    sys.exit(main())
