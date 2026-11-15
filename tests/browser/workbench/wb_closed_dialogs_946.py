"""Measure what the closed dialogs of the workbench cost (#946).

Two numbers, on one page load with every dialog closed:
- the live Alpine effects: the sum of `el._x_effects.size` over every element,
  split by the `.modal-scrim` that holds the element (or `page` for the rest);
- one store change: `Alpine.store('projects').setProjects(copy)` with a copy of
  each project, timed with `performance.now()` from the call to the end of the
  next Alpine tick and to the next animation frame, plus the CDP
  `Performance.getMetrics` script and layout time across the same change.

Boots a debug daemon on port 7484 over a SCRATCH `RALPHY_DAEMON_DIR` with
PROJECTS fixture repos, stops it by its own handle, never by name.

Run from the repo root (build target/debug/ralphy.exe first):
    python tests/browser/workbench/wb_closed_dialogs_946.py measure <variant> [runs]
    python tests/browser/workbench/wb_closed_dialogs_946.py patch
`measure` writes .ralphy/closed-dialogs/<variant>.json. `patch` puts the body
of each `.modal-scrim` inside `<template x-if>` of its open state, in the UI
sources: rebuild, measure, then `git checkout -- crates/ralphy-daemon/assets/ui`.
The patch is never committed.
"""
import json, os, re, socket, statistics, subprocess, sys, tempfile, time
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")
REPO = Path(__file__).resolve().parents[3]
UI = REPO / "crates" / "ralphy-daemon" / "assets" / "ui"
OUT = REPO / ".ralphy" / "closed-dialogs"
PORT = 7484
BASE = f"http://127.0.0.1:{PORT}/"
TARGET = REPO / "target" / "debug"
EXE = str(TARGET / "ralphy.exe")
CHILD = str(TARGET / "session_test_child.exe")
PROJECTS = 30
CHANGES = 20
SCRIM = re.compile(r"""^(\s*)<div class="modal-scrim" x-cloak x-bind="\$store\.modals\.scrim\('([^']+)'""")

COUNT_JS = """() => {
  const out = { page: 0 };
  const bindings = { page: 0 };
  for (const el of document.querySelectorAll('*')) {
    const scrim = el.closest('.modal-scrim');
    const key = scrim ? scrim.getAttribute('x-bind').match(/scrim\\('([^']+)'/)[1] : 'page';
    out[key] = (out[key] || 0) + (el._x_effects ? el._x_effects.size : 0);
    const n = [...el.attributes].filter((a) => /^(x-|:|@)/.test(a.name)).length;
    bindings[key] = (bindings[key] || 0) + n;
  }
  const total = Object.values(out).reduce((a, b) => a + b, 0);
  const elements = document.querySelectorAll('*').length;
  return { effects: out, bindings, total, elements };
}"""

CHANGE_JS = """async () => {
  const s = Alpine.store('projects');
  const copy = s.projects.map((p) => ({ ...p }));
  const t0 = performance.now();
  s.setProjects(copy);
  const tSync = performance.now();
  await Alpine.nextTick();
  const tTick = performance.now();
  await new Promise((r) => requestAnimationFrame(() => r()));
  const tFrame = performance.now();
  return { sync: tSync - t0, tick: tTick - t0, frame: tFrame - t0, n: copy.length };
}"""


def patch():
    changed = 0
    for f in sorted(UI.glob("*.part.html")):
        lines = f.read_text(encoding="utf-8").split("\n")
        out, i = [], 0
        while i < len(lines):
            m = SCRIM.match(lines[i])
            out.append(lines[i])
            if not m:
                i += 1
                continue
            ind, expr = len(m.group(1)), m.group(2)
            out.append(f'{m.group(1)}<template x-if="{expr}"><div style="display: contents">')
            j = i + 1
            while not (lines[j].strip() == "</div>" and len(lines[j]) - len(lines[j].lstrip()) == ind):
                out.append(lines[j])
                j += 1
            out.append(f"{m.group(1)}</div></template>")
            out.append(lines[j])
            changed += 1
            i = j + 1
        with open(f, "w", encoding="utf-8", newline="\n") as h:
            h.write("\n".join(out))
    print("wrapped", changed, "dialog bodies")


def env(d):
    e = tempfile.mkdtemp(prefix="cd946_empty_")
    return dict(os.environ, RALPHY_DAEMON_DIR=d, RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
                RALPHY_USAGE_DIR=e, RALPHY_CLAUDE_PROJECTS_DIR=e, RALPHY_CODEX_DIR=e,
                RALPHY_OPENCODE_DB=os.path.join(e, "none.db"), RALPHY_KIMI_DIR=e,
                RALPHY_KIMI_CODE_DIR=e)


def fixture(root, i):
    d = Path(root, f"proj{i:02d}")
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# fixture {i}\n", encoding="utf-8")
    for a in (["git", "init", "-b", "main"], ["git", "config", "user.email", "d@e.x"],
              ["git", "config", "user.name", "d"], ["git", "add", "-A"],
              ["git", "commit", "-m", "f"]):
        subprocess.run(a, cwd=d, check=True, capture_output=True)
    return str(d)


def listening():
    s = socket.socket(); s.settimeout(0.5)
    try:
        return s.connect_ex(("127.0.0.1", PORT)) == 0
    finally:
        s.close()


def metrics(cdp):
    m = {x["name"]: x["value"] for x in cdp.send("Performance.getMetrics")["metrics"]}
    return {k: m.get(k, 0.0) for k in ("ScriptDuration", "LayoutDuration", "RecalcStyleDuration",
                                       "TaskDuration")}


def one_run(b):
    ctx = b.new_context(viewport={"width": 1440, "height": 900})
    p = ctx.new_page()
    errors = []
    p.on("pageerror", lambda e: errors.append(str(e)))
    cdp = ctx.new_cdp_session(p)
    cdp.send("Performance.enable")
    p.goto(BASE)
    p.wait_for_function("() => window.Alpine && Alpine.store('connection').authed", timeout=20000)
    p.wait_for_function(f"() => Alpine.store('projects').projects.length >= {PROJECTS}",
                        timeout=20000)
    p.wait_for_timeout(2500)
    count = p.evaluate(COUNT_JS)
    changes = []
    for _ in range(CHANGES):
        before = metrics(cdp)
        t = p.evaluate(CHANGE_JS)
        after = metrics(cdp)
        t.update({k: (after[k] - before[k]) * 1000 for k in before})
        changes.append(t)
        p.wait_for_timeout(100)
    ctx.close()
    return {"count": count, "changes": changes, "errors": errors}


def summary(runs):
    out = {}
    keys = ["sync", "tick", "frame", "ScriptDuration", "LayoutDuration", "RecalcStyleDuration",
            "TaskDuration"]
    for k in keys:
        vals = [c[k] for r in runs for c in r["changes"]]
        q = statistics.quantiles(vals, n=4)
        out[k] = {"median": statistics.median(vals), "p25": q[0], "p75": q[2],
                  "min": min(vals), "max": max(vals), "n": len(vals)}
    totals = [r["count"]["total"] for r in runs]
    out["effects_total"] = {"median": statistics.median(totals), "min": min(totals),
                            "max": max(totals)}
    return out


def measure(variant, n):
    from playwright.sync_api import sync_playwright
    if listening():
        print("port busy"); sys.exit(1)
    d = tempfile.mkdtemp(prefix="cd946_store_")
    root = tempfile.mkdtemp(prefix="cd946_fix_")
    for i in range(PROJECTS):
        subprocess.run([EXE, "daemon", "add", fixture(root, i)],
                       env=dict(os.environ, RALPHY_DAEMON_DIR=d), check=True, capture_output=True)
    proc = subprocess.Popen([EXE, "daemon", "--port", str(PORT)], env=env(d),
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    runs = []
    try:
        for _ in range(60):
            if listening():
                break
            time.sleep(0.5)
        with sync_playwright() as pw:
            b = pw.chromium.launch()
            for i in range(n):
                runs.append(one_run(b))
                print("run", i + 1, "of", n, "done", flush=True)
            b.close()
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
    OUT.mkdir(parents=True, exist_ok=True)
    data = {"variant": variant, "projects": PROJECTS, "runs": runs, "summary": summary(runs)}
    with open(OUT / f"{variant}.json", "w", encoding="utf-8", newline="\n") as h:
        h.write(json.dumps(data, indent=1))
    print(json.dumps(data["summary"], indent=1))
    print("effects by dialog (run 1):", json.dumps(runs[0]["count"]["effects"]))
    print("errors (run 1):", runs[0]["errors"][:5])


if __name__ == "__main__":
    if sys.argv[1] == "patch":
        patch()
    else:
        measure(sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 5)
