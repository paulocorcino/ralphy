#!/usr/bin/env python3
"""The burndown of a lint transition plan: what the plan measured, what is left.

  progress.py record RECORD --sha SHA --date DATE --phases PHASES.json
                     --run NAME TALLY COMMAND [--run ...] [--tool NAME=VERSION ...] [--draft PATH ...]
                     [--layout LAYOUT.json]
  progress.py check RECORD [--plan PLAN]
  progress.py commands RECORD
  progress.py show RECORD PHASE --plan PLAN
  progress.py decide RECORD ID ANSWER --date DATE
  progress.py issue RECORD ID URL --date DATE
  progress.py note RECORD TEXT --date DATE
  progress.py log RECORD
  progress.py point RECORD --run NAME TALLY [--run ...] [--tool NAME=VERSION ...]
                    --sha SHA --date DATE [--config CONFIG ...] [--append]

`record` writes the record of the measure the plan was cut from: per run, the
command and the sites per rule (from the `tally.py --json` file of that run);
the tool versions; a hash of each draft config the commands read; and the phase
of each rule (PHASES: {"<rule as tally prints it>": "<phase>"}). It fails when
a rule with sites has no phase. Run it from the repo root; draft paths are
relative to it. NAME is the tool's name, as in --tool, so a version change
is tied to its run.

LAYOUT is the shape of the plan: {"phases": {"<id>": {"lane": "<lane>", "waits":
["<id>", ...]}}, "decisions": {"<id>": {"holds": ["<phase id>", ...]}}}; every
phase of PHASES is in it, and a phase with no rule (Phase 0) is in it too. A
record written again without --layout keeps the layout it had.

`check` fails when a rule with sites has no phase, or a phase names a rule the
record does not hold. With --plan it also reads the plan: every section of the
contract is there, each phase of the layout has its `### Phase <id>` heading and
each such heading a phase in the layout, each decision of the layout is named
in the plan, and the last line is the end marker, so a plan cut short fails.
`commands` prints each run's command, to measure again.

The state of the plan lives in the history file, one JSON line per event, and
only these commands write it, by appending: `decide` records the answer to a
decision, `issue` the issue opened for a phase or a decision, `note` a line of
progress (a slice landed, a flip). `log` prints the state: each decision open
or answered, each issue, then the notes and the points in date order. `show`
prints one phase for the agent that works it: its section of the plan, its
lane and the state of each phase it waits for, its rules with the counts of
the record and of the last point, each decision that holds it, and its issue.

`point` compares a new measure, one tally file per run of the record, with the
record and with the last point. Per rule: `closed` (no site left), `down`,
`flat`, `up` (the debt grew while the rule is off), and for a rule with no site
in the record: `off in the repo` when the tool versions and the drafts are the
record's (the rule was on, so the gate would fail on its sites: it was turned
off), or `scope` when a version or a draft changed (a rule the record never
measured). With --config (each config that holds a debt list), a rule
`closed` that its config still lists as debt is `ready to flip`: the next
change deletes its entry. Per phase: `closed`, `in progress`, `not started`, or `regressed`.
--append adds the point to the history (`<RECORD stem>.history.jsonl`). It exits 1 on `up` or
`off in the repo`, after printing everything.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from debt import entries  # noqa: E402  (a sibling script, not a package)


def bare(rule: str) -> str:
    """The rule's own name: `clippy::foo`, `eslint(no-foo)`, `eslint/no-foo` -> `foo`, `no-foo`."""
    return [p for p in re.split(r"::|/|\(|\)", rule) if p][-1]


def sha256(path: str) -> str:
    return hashlib.sha256(Path(path).read_bytes().replace(b"\r\n", b"\n")).hexdigest()[:16]


def counts_of(tally: str) -> dict[str, int]:
    data = json.loads(Path(tally).read_text(encoding="utf-8"))
    return {rule: len(sites) for rule, sites in data.items() if sites}


def pairs(items: list[str] | None, what: str) -> dict[str, str]:
    out = {}
    for item in items or []:
        name, sep, value = item.partition("=")
        if not sep:
            sys.exit(f"{what} takes NAME=VALUE, got {item!r}")
        out[name] = value
    return out


def load(path: str) -> dict:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def history_path(record: str) -> Path:
    p = Path(record)
    return p.with_name(p.stem + ".history.jsonl")


def history(record: str) -> list[dict]:
    hist = history_path(record)
    if not hist.exists():
        return []
    return [json.loads(line) for line in hist.read_text(encoding="utf-8").splitlines() if line.strip()]


def last_point(record: str) -> dict | None:
    points = [e for e in history(record) if "kind" not in e]
    return points[-1] if points else None


def append_event(record: str, event: dict) -> None:
    with history_path(record).open("a", encoding="utf-8", newline="\n") as fh:
        fh.write(json.dumps(event, sort_keys=True, ensure_ascii=False) + "\n")


def phase_of(phases: dict[str, str], run: str, rule: str) -> str | None:
    return phases.get(f"{run}:{rule}") or phases.get(rule)


def coverage(rec: dict) -> list[str]:
    phases = rec["phases"]
    bad = [f"no phase: {run}:{rule} ({n} sites)"
           for run, r in rec["runs"].items() for rule, n in sorted(r["counts"].items())
           if not phase_of(phases, run, rule)]
    held = {rule for r in rec["runs"].values() for rule in r["counts"]}
    held |= {f"{run}:{rule}" for run, r in rec["runs"].items() for rule in r["counts"]}
    bad += [f"phase {p} names {rule}, which has no site in the record" for rule, p in sorted(phases.items())
            if rule not in held]
    layout = rec.get("layout")
    if layout:
        ids = layout.get("phases", {})
        bad += [f"phase {p} has rules but is not in the layout" for p in sorted(set(phases.values()) - set(ids))]
        for p, meta in sorted(ids.items()):
            bad += [f"phase {p} waits for phase {w}, which is not in the layout" for w in meta.get("waits", [])
                    if w not in ids]
        for d, meta in sorted(layout.get("decisions", {}).items()):
            bad += [f"decision {d} holds phase {h}, which is not in the layout" for h in meta.get("holds", [])
                    if h not in ids]
    return bad


SECTIONS = ["Working this plan", "Target", "Decisions for the human", "Phases", "One-way doors", "Fix discipline"]
END = "<!-- end of the plan: progress.py check reads this line -->"
_PHASE = re.compile(r"^(#{3,4}) Phase ([\w.-]+)\b.*$", re.M)


def phase_sections(text: str) -> dict[str, str]:
    """Each `### Phase <id>` heading and its text, up to the next heading of its level or above."""
    out = {}
    for m in _PHASE.finditer(text):
        level = len(m.group(1))
        rest = text[m.end():]
        stop = re.search(rf"^#{{1,{level}}} ", rest, re.M)
        out[m.group(2)] = m.group(0) + (rest[:stop.start()] if stop else rest)
    return out


def plan_problems(rec: dict, plan: str) -> list[str]:
    text = Path(plan).read_text(encoding="utf-8-sig")
    bad = [f"no section `{name}`: the plan is cut short or a section is missing" for name in SECTIONS
           if not re.search(rf"^#{{2,3}} {re.escape(name)}\s*$", text, re.M)]
    lines = [line for line in text.splitlines() if line.strip()]
    if not lines or lines[-1].strip() != END:
        bad.append(f"the last line is not the end marker: the plan is cut short ({len(lines)} lines read)")
    layout = rec.get("layout")
    if not layout:
        bad.append("the record has no layout: write it again with --layout")
        return bad
    found = set(phase_sections(text))
    ids = set(layout.get("phases", {}))
    bad += [f"phase {p} is in the layout but the plan has no `### Phase {p}` heading" for p in sorted(ids - found)]
    bad += [f"the plan has `### Phase {p}`, which is not in the layout" for p in sorted(found - ids)]
    bad += [f"decision {d} is in the layout but the plan never names it" for d in sorted(layout.get("decisions", {}))
            if not re.search(rf"\b{re.escape(d)}\b", text)]
    return bad


def cmd_record(a: argparse.Namespace) -> int:
    runs = {name: {"command": command, "counts": counts_of(tally)} for name, tally, command in a.run}
    rec = {
        "sha": a.sha, "date": a.date,
        "tools": pairs(a.tool, "--tool"),
        "drafts": {d: sha256(d) for d in a.draft or []},
        "runs": runs,
        "phases": load(a.phases),
    }
    if a.layout:
        rec["layout"] = load(a.layout)
    elif Path(a.record).exists():
        kept = load(a.record).get("layout")
        if kept:
            rec["layout"] = kept
    bad = coverage(rec)
    Path(a.record).write_text(json.dumps(rec, indent=1, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    total = sum(sum(r["counts"].values()) for r in runs.values())
    print(f"record: {len(runs)} runs, {total} sites, {len(rec['phases'])} rules with a phase")
    for line in bad:
        print(line)
    return 1 if bad else 0


def cmd_check(a: argparse.Namespace) -> int:
    rec = load(a.record)
    bad = coverage(rec)
    if a.plan:
        bad += plan_problems(rec, a.plan)
    for line in bad:
        print(line)
    ok = "every rule with sites has a phase" + (", and the plan is whole and matches the layout" if a.plan else "")
    print(ok if not bad else f"{len(bad)} problems")
    return 1 if bad else 0


def known_id(rec: dict, ident: str, kinds: tuple[str, ...]) -> bool:
    layout = rec.get("layout") or {}
    return any(ident in layout.get(k, {}) for k in kinds)


def cmd_decide(a: argparse.Namespace) -> int:
    if not known_id(load(a.record), a.id, ("decisions",)):
        print(f"{a.id} is not a decision of the layout")
        return 1
    append_event(a.record, {"kind": "decision", "id": a.id, "answer": a.answer, "date": a.date})
    print(f"{a.id} answered")
    return 0


def cmd_issue(a: argparse.Namespace) -> int:
    if not known_id(load(a.record), a.id, ("phases", "decisions")):
        print(f"{a.id} is not a phase or a decision of the layout")
        return 1
    append_event(a.record, {"kind": "issue", "id": a.id, "url": a.url, "date": a.date})
    print(f"issue of {a.id}: {a.url}")
    return 0


def cmd_note(a: argparse.Namespace) -> int:
    append_event(a.record, {"kind": "note", "text": a.text, "date": a.date})
    print("noted")
    return 0


def state(record: str) -> tuple[dict[str, dict], dict[str, str]]:
    """The answers (id -> event) and the issues (id -> url); the last event of each wins."""
    answers, issues = {}, {}
    for e in history(record):
        if e.get("kind") == "decision":
            answers[e["id"]] = e
        elif e.get("kind") == "issue":
            issues[e["id"]] = e["url"]
    return answers, issues


def cmd_log(a: argparse.Namespace) -> int:
    rec = load(a.record)
    answers, issues = state(a.record)
    layout = rec.get("layout") or {}
    print("decisions:")
    for d, meta in sorted(layout.get("decisions", {}).items()):
        holds = ", ".join(meta.get("holds", [])) or "nothing"
        e = answers.get(d)
        said = f"answered {e['date']}: {e['answer']}" if e else "open"
        print(f"  {d} (holds {holds}): {said}" + (f"  {issues[d]}" if d in issues else ""))
    print("issues:")
    for p in layout.get("phases", {}):
        if p in issues:
            print(f"  Phase {p}: {issues[p]}")
    total = sum(sum(r["counts"].values()) for r in rec["runs"].values())
    lines = [(rec["date"], f"{rec['date']} {rec['sha']}: record, {total} sites")]
    for e in history(a.record):
        if "kind" not in e:
            lines.append((e["date"], f"{e['date']} {e['sha']}: point, {e['total']} sites left"
                          + (f", {e['scope']} of scope" if e.get("scope") else "")))
        elif e["kind"] == "note":
            lines.append((e["date"], f"{e['date']}: {e['text']}"))
    print("history:")
    for _, line in sorted(lines, key=lambda x: x[0]):  # stable: one date keeps the order written
        print(f"  {line}")
    return 0


def phase_state(rec: dict, counts: dict[str, int] | None, phase: str) -> tuple[str, list[tuple[str, int, int]]]:
    rules = [(f"{run}:{rule}", n, n if counts is None else counts.get(f"{run}:{rule}", 0))
             for run, r in rec["runs"].items() for rule, n in sorted(r["counts"].items())
             if phase_of(rec["phases"], run, rule) == phase]
    if not rules:
        return "no rule: read by its Done when", rules
    left = sum(1 for _, _, now in rules if now)
    verdict = ("closed" if not left else "not started" if all(now >= then for _, then, now in rules)
               else "in progress")
    return f"{verdict} ({len(rules) - left} of {len(rules)} rules at 0)", rules


def cmd_show(a: argparse.Namespace) -> int:
    rec = load(a.record)
    layout = rec.get("layout") or {}
    phases = layout.get("phases", {})
    if a.phase not in phases:
        print(f"phase {a.phase} is not in the layout; the phases are: {', '.join(phases)}")
        return 1
    sections = phase_sections(Path(a.plan).read_text(encoding="utf-8-sig"))
    if a.phase not in sections:
        print(f"the plan has no `### Phase {a.phase}` heading: run `check --plan`")
        return 1
    point = last_point(a.record)
    counts = point["counts"] if point else None
    answers, issues = state(a.record)
    print(sections[a.phase].rstrip() + "\n")
    print("--- state, from the " + (f"last point {point['date']} {point['sha']}" if point else "record (no point yet)"))
    meta = phases[a.phase]
    print(f"lane: {meta.get('lane', '-')}" + (f"; issue: {issues[a.phase]}" if a.phase in issues else ""))
    for w in meta.get("waits", []):
        print(f"waits for Phase {w}: {phase_state(rec, counts, w)[0]}")
    verdict, rules = phase_state(rec, counts, a.phase)
    print(f"this phase: {verdict}")
    for rule, then, now in rules:
        print(f"  {rule}: record {then}, now {now}")
    for d, dm in sorted(layout.get("decisions", {}).items()):
        if a.phase in dm.get("holds", []):
            e = answers.get(d)
            print(f"held by {d}: " + (f"answered {e['date']}: {e['answer']}" if e
                                      else "open, the slices it holds wait"))
    print("the counts are the last measure; the phase's Measure first commands give today's")
    return 0


def cmd_commands(a: argparse.Namespace) -> int:
    rec = load(a.record)
    for name, r in rec["runs"].items():
        print(f"{name}: {r['command']}")
    if rec["tools"]:
        print("at " + ", ".join(f"{k} {v}" for k, v in sorted(rec["tools"].items())))
    return 0


def state_of(then: int, now: int, same: bool) -> str:
    if then == 0:
        return "off in the repo" if same else "scope"
    if now == 0:
        return "closed"
    return "down" if now < then else "up" if now > then else "flat"


def cmd_point(a: argparse.Namespace) -> int:
    rec = load(a.record)
    given = {name: tally for name, tally in a.run}
    missing = [r for r in rec["runs"] if r not in given]
    extra = [r for r in given if r not in rec["runs"]]
    if missing or extra:
        for r in missing:
            print(f"run {r} not measured: every run of the record is measured again, or the point says nothing about its rules")
        for r in extra:
            print(f"run {r} is not in the record")
        return 1

    tools = pairs(a.tool, "--tool")
    bumped = {k: (v, tools.get(k)) for k, v in rec["tools"].items() if tools.get(k) != v}
    drafts = {d: h for d, h in rec["drafts"].items() if not Path(d).exists() or sha256(d) != h}
    hist = history_path(a.record)
    last = last_point(a.record)

    listed: dict[str, int] = {}
    for cfg in a.config or []:
        debt, _noise, _, _plain = entries(Path(cfg).read_text(encoding="utf-8-sig"))
        listed |= {bare(r): n for r, n in debt.items()}

    rows = []  # run, rule, phase, then, last, now, state
    for run, r in rec["runs"].items():
        now = counts_of(given[run])
        # A run is named for its tool (`--run ruff ...`, `--tool ruff=0.17.4`); a run whose
        # name is not a tool is held to every version.
        same = not drafts and (run not in bumped if run in rec["tools"] else not bumped)
        for rule in sorted(set(r["counts"]) | set(now)):
            then, n = r["counts"].get(rule, 0), now.get(rule, 0)
            prev = None if last is None else last.get("counts", {}).get(f"{run}:{rule}", 0)
            state = state_of(then, n, same)
            if state == "closed" and bare(rule) in listed:
                state = "ready to flip"
            rows.append((run, rule, phase_of(rec["phases"], run, rule) or "-", then, prev, n, state))

    order = {"up": 0, "off in the repo": 1, "scope": 2, "ready to flip": 3, "flat": 4, "down": 5, "closed": 6}
    rows.sort(key=lambda x: (order[x[6]], -x[5], x[0], x[1]))
    w = max([len(f"{x[0]}:{x[1]}") for x in rows] + [4])
    print(f"{'rule':<{w}}  {'phase':>6}  {'record':>6}  {'last':>6}  {'now':>6}  state")
    for run, rule, phase, then, prev, n, state in rows:
        if state == "flat" and not a.all:
            continue
        print(f"{run + ':' + rule:<{w}}  {phase:>6}  {then:>6}  {'' if prev is None else prev:>6}  {n:>6}  {state}")
    flat = sum(1 for x in rows if x[6] == "flat")
    if flat and not a.all:
        print(f"({flat} rules flat; --all lists them)")

    phases: dict[str, list[str]] = {}
    for x in rows:
        if x[3] > 0:
            phases.setdefault(x[2], []).append(x[6])
    print(f"\n{'phase':>6}  state")
    for p in sorted(phases, key=lambda s: (len(s), s)):
        st = phases[p]
        done = sum(1 for s in st if s in ("closed", "ready to flip"))
        verdict = ("regressed" if "up" in st else "closed" if all(s == "closed" for s in st)
                   else "ready to flip" if done == len(st)
                   else "in progress" if done or "down" in st else "not started")
        print(f"{p:>6}  {verdict} ({done} of {len(st)} rules at 0)")

    then_total = sum(x[3] for x in rows)
    now_total = sum(x[5] for x in rows if x[6] not in ("scope",))
    scope_total = sum(x[5] for x in rows if x[6] == "scope")
    print(f"\nrecord {rec['sha']} {rec['date']}: {then_total} sites; now {a.sha} {a.date}: {now_total} sites"
          f" ({now_total - then_total:+d} net)" + (f", plus {scope_total} sites of scope" if scope_total else "")
          + (f"; last point {last['sha']} {last['date']}: {last['total']}" if last else ""))
    for k, (old, new) in bumped.items():
        print(f"version changed: {k} {old} -> {new or 'not given'}: its counts mix fixes and the bump")
    for d in drafts:
        print(f"draft changed since the record: {d}: its runs' new rules are scope, not erosion")

    stale = [(f"{x[0]}:{x[1]}", listed[bare(x[1])], x[5]) for x in rows
             if x[6] in ("down", "flat") and bare(x[1]) in listed and listed[bare(x[1])] > x[5]]
    for rule, said, n in stale:
        print(f"debt comment above the count: {rule} says {said}, now {n}: the next change of its phase lowers it")
    if last and a.date < last["date"]:
        print(f"warning: this point ({a.date}) is dated before the last one ({last['date']}): check the date")

    bad = [x for x in rows if x[6] in ("up", "off in the repo")]
    for run, rule, *_ , state in bad:
        print(f"{state}: {run}:{rule}" + (" (the debt grew while the rule is off)" if state == "up"
                                           else " (no site in the record and the same versions: the rule was turned off in the repo)"))
    if a.append:
        point = {"sha": a.sha, "date": a.date, "total": now_total, "scope": scope_total,
                 "counts": {f"{x[0]}:{x[1]}": x[5] for x in rows if x[5]}}
        with hist.open("a", encoding="utf-8", newline="\n") as fh:
            fh.write(json.dumps(point, sort_keys=True) + "\n")
        closed = sum(1 for x in rows if x[6] in ("closed", "ready to flip"))
        print(f"\npoint appended: {a.date} {a.sha}: {now_total} of {then_total} sites left, "
              f"{closed} rules at 0" + (f", {scope_total} sites of scope" if scope_total else "")
              + (f", {len(bad)} regressions" if bad else ""))
    return 1 if bad else 0


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")  # the plan's text, on any console
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("record")
    r.add_argument("record")
    r.add_argument("--sha", required=True)
    r.add_argument("--date", required=True)
    r.add_argument("--phases", required=True)
    r.add_argument("--run", nargs=3, action="append", required=True, metavar=("NAME", "TALLY", "COMMAND"))
    r.add_argument("--tool", action="append")
    r.add_argument("--draft", action="append")
    r.add_argument("--layout")
    c = sub.add_parser("check")
    c.add_argument("record")
    c.add_argument("--plan")
    sh = sub.add_parser("show")
    sh.add_argument("record")
    sh.add_argument("phase")
    sh.add_argument("--plan", required=True)
    d = sub.add_parser("decide")
    d.add_argument("record")
    d.add_argument("id")
    d.add_argument("answer")
    d.add_argument("--date", required=True)
    i = sub.add_parser("issue")
    i.add_argument("record")
    i.add_argument("id")
    i.add_argument("url")
    i.add_argument("--date", required=True)
    n = sub.add_parser("note")
    n.add_argument("record")
    n.add_argument("text")
    n.add_argument("--date", required=True)
    lg = sub.add_parser("log")
    lg.add_argument("record")
    m = sub.add_parser("commands")
    m.add_argument("record")
    p = sub.add_parser("point")
    p.add_argument("record")
    p.add_argument("--run", nargs=2, action="append", required=True, metavar=("NAME", "TALLY"))
    p.add_argument("--tool", action="append")
    p.add_argument("--sha", required=True)
    p.add_argument("--date", required=True)
    p.add_argument("--config", action="append", help="a config that holds a debt list")
    p.add_argument("--append", action="store_true")
    p.add_argument("--all", action="store_true", help="list the flat rules too")
    a = ap.parse_args()
    return {"record": cmd_record, "check": cmd_check, "commands": cmd_commands, "point": cmd_point,
            "show": cmd_show, "decide": cmd_decide, "issue": cmd_issue, "note": cmd_note,
            "log": cmd_log}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
