"""#487 part 3: what the repeated view calls cost on a real project.

Alpine keeps no computed cache: a method the template reads at N places runs N
times per change. This script measures whether that shows in the time a board
refresh and a project switch spend in script. It asserts nothing; it prints
numbers for the issue.

A daemon on a scratch `RALPHY_DAEMON_DIR` (own port, own registry) serves two
REAL repositories, given on the command line, with the real usage stores, so
the Spend tab reads a full ledger. The first board fold goes to GitHub; its
reply is then replayed inside the page, so a measured refresh is the render
work only, not the network.

Each measured function is wrapped with a call counter and a timer. The
interaction's own script time is Chrome's `ScriptDuration` (CDP Performance
domain), read before and after, so the share of the view functions in it is
a plain ratio.

Run: python tests/browser/workbench/wb_perf_487.py <repo-a> <repo-b>
"""

import json
import os
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7489
BASE = f"http://127.0.0.1:{PORT}/"
REPEAT = 5

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

# The wrappers. `window.__perf` holds, per name, the calls and the time since
# the last reset. The shell methods are replaced on the component, so the
# template's next read reaches the wrapper; `this` is passed through.
INSTRUMENT = """
() => {
  const sh = SH_EXPR;
  window.__perf = {};
  // A view that returns a `kind` is also counted per kind, so the numbers say
  // whether the call built the full view or returned early.
  const wrap = (name, fn) => function (...args) {
    const t0 = performance.now();
    let out;
    try { out = fn.apply(this, args); return out; }
    finally {
      const p = (window.__perf[name] ||= { calls: 0, ms: 0 });
      p.calls += 1; p.ms += performance.now() - t0;
      if (out && out.kind) (window.__perf[`${name} [${out.kind}]`] ||= { calls: 0, ms: 0 }).calls += 1;
    }
  };
  window.WBSpend.state = wrap('WBSpend.state', window.WBSpend.state);
  window.WBSpend.ledger = wrap('WBSpend.ledger', window.WBSpend.ledger);
  for (const m of ['kanbanColumns', 'selectedIssue', 'currentRun']) sh[m] = wrap(m, sh[m]);
}
"""

# Replay the last real `board.list` reply for its repo, instantly.
REPLAY = """
() => {
  const real = window.WBDaemon.observe;
  window.__boards = window.__boards || {};
  window.WBDaemon.observe = async (verb, payload) => {
    if (verb === 'board.list' && window.__replay && window.__boards[payload.repo]) {
      return window.__boards[payload.repo];
    }
    const reply = await real(verb, payload);
    if (verb === 'board.list') window.__boards[payload.repo] = reply;
    return reply;
  };
}
"""


def wait_listening(base, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def port_free(port):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(0.5)
    try:
        return s.connect_ex(("127.0.0.1", port)) != 0
    finally:
        s.close()


def register(daemon_dir, repo):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    out = subprocess.run([EXE, "daemon", "add", repo], env=env, check=True, capture_output=True, encoding="utf-8")
    return out.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def idle(page, ms=400):
    page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")
    page.wait_for_timeout(ms)


def load_spend(page, slug):
    """Open the Spend tab on `slug` and wait for its document and its ledger."""
    page.evaluate(f"() => {SH}.openSpend()")
    page.wait_for_function(
        f"(s) => {SH}.spend.slug === s && !{SH}.spend.loading && !!{SH}.spend.doc", arg=slug, timeout=180000
    )
    page.evaluate(f"() => {SH}.setSpendPane('ledger')")
    page.wait_for_function(f"(s) => {SH}.ledger.slug === s && !{SH}.ledger.loading", arg=slug, timeout=180000)
    page.evaluate(f"() => {SH}.setSpendPane('overview')")


def script_s(cdp):
    metrics = cdp.send("Performance.getMetrics")["metrics"]
    return next(m["value"] for m in metrics if m["name"] == "ScriptDuration")


def measure(page, cdp, act, settle_ms):
    page.evaluate("() => { window.__perf = {}; }")
    before = script_s(cdp)
    page.evaluate(act)
    idle(page, settle_ms)
    script_ms = (script_s(cdp) - before) * 1000
    perf = page.evaluate("() => window.__perf")
    return script_ms, perf


def summarize(label, runs):
    names = sorted({n for _, p in runs for n in p})
    script = statistics.median(s for s, _ in runs)
    views = statistics.median(sum(v["ms"] for v in p.values()) for _, p in runs)
    print(f"\n## {label}  (median of {len(runs)})")
    print(f"script time: {script:.1f} ms; the measured view functions: {views:.1f} ms "
          f"({(100 * views / script) if script else 0:.1f} %)")
    for n in names:
        calls = statistics.median(p.get(n, {"calls": 0})["calls"] for _, p in runs)
        ms = statistics.median(p.get(n, {"ms": 0})["ms"] for _, p in runs)
        print(f"  {n:<16} {calls:>6.0f} calls  {ms:>8.2f} ms")
    return {"script_ms": script, "views_ms": views}


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    repo_a, repo_b = (os.path.abspath(a) for a in sys.argv[1:])
    if not port_free(PORT):
        print(f"port {PORT} is taken")
        sys.exit(1)
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    daemon_dir = tempfile.mkdtemp(prefix="wb487perf_")
    slug_a, slug_b = register(daemon_dir, repo_a), register(daemon_dir, repo_b)
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    out = {}
    try:
        if not wait_listening(BASE):
            print("daemon not listening")
            sys.exit(1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_context(viewport={"width": 1440, "height": 900}).new_page()
            cdp = page.context.new_cdp_session(page)
            cdp.send("Performance.enable")
            page.goto(BASE)
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=30000)
            page.evaluate(REPLAY)

            # Open A with its board, its spend document and its ledger loaded.
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_a)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug_a, timeout=30000)
            page.evaluate(f"() => {SH}.toggleKanban()")
            page.wait_for_function(f"(s) => ({SH}.boardIssues[s] || []).length > 0", arg=slug_a, timeout=180000)
            load_spend(page, slug_a)
            # B's board once for real, so the switch back and forth replays both.
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_b)
            page.wait_for_function(f"(s) => ({SH}.boardIssues[s] !== undefined)", arg=slug_b, timeout=180000)
            page.wait_for_function(f"() => !{SH}.boardRefreshing", timeout=180000)
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_a)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug_a, timeout=30000)
            idle(page, 1500)

            facts = page.evaluate(
                f"(s) => ({{ issues: ({SH}.boardIssues[s] || []).length,"
                f"  deliveries: ({SH}.spend.doc?.deliveries || []).length,"
                f"  records: ({SH}.ledger.records || []).length,"
                f"  interactive: ({SH}.ledger.interactive || []).length,"
                f"  runs: ({SH}.runsByProject[s] || []).length }})",
                arg=slug_a,
            )
            print("project A:", slug_a, json.dumps(facts))
            page.evaluate(INSTRUMENT.replace("SH_EXPR", SH))
            page.evaluate("() => { window.__replay = true; }")
            idle(page, 500)

            # The cost of ONE call, from a loop: `performance.now()` is coarse
            # outside a cross-origin-isolated page, so a single short call
            # reads as 0 ms.
            load_spend(page, slug_a)
            page.evaluate(f"() => {{ {SH}.kanbanSel = ({SH}.boardIssues[{SH}.openSlug] || [])[0]?.number ?? null; }}")
            per_call = page.evaluate(
                f"() => {{ const sh = {SH}; const out = {{}};"
                "  for (const m of ['spendView', 'ledgerView', 'kanbanColumns', 'selectedIssue', 'currentRun']) {"
                "    const t0 = performance.now();"
                "    for (let i = 0; i < 200; i++) sh[m]();"
                "    out[m] = (performance.now() - t0) / 200; }"
                "  return out; }"
            )
            page.evaluate(f"() => {{ {SH}.kanbanSel = null; }}")
            print("\n## one call, mean of 200 (ms)")
            for name, ms in per_call.items():
                print(f"  {name:<16} {ms:.3f}")
            out["one call, ms"] = per_call

            for tab in ("consoles", "spend"):
                # The spend document is per project, and a switch drops it:
                # read it again so the refresh below runs over a full one.
                load_spend(page, slug_a)
                page.evaluate(f"(t) => {{ {SH}.active = t; }}", arg=tab)
                idle(page, 800)
                refresh = [measure(page, cdp, f"() => {SH}.loadBoard()", 600) for _ in range(REPEAT)]
                out[f"board refresh, {tab} tab shown"] = summarize(f"board refresh, {tab} tab shown", refresh)
                switch = []
                for _ in range(REPEAT):
                    for slug in (slug_b, slug_a):
                        switch.append(measure(page, cdp, f"() => {SH}.toggle({json.dumps(slug)})", 1500))
                out[f"project switch, {tab} tab shown"] = summarize(f"project switch, {tab} tab shown", switch)
            browser.close()
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
    print("\n" + json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
