"""Run every tests/browser script one at a time and record the result.

Output: .ralphy/browser-sweep/results.jsonl (one line per script) and
.ralphy/browser-sweep/logs/<area>__<name>.log (full output). A script already
in results.jsonl is not run again: delete the file to start over.

    python tests/browser/sweep.py [--list] [part of a path ...]

--list prints each script, then `run` or `skipped` and runs nothing.
"""
import glob
import json
import os
import re
import subprocess
import sys
import tempfile
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(ROOT, ".ralphy", "browser-sweep")
LOGS = os.path.join(OUT, "logs")
RESULTS = os.path.join(OUT, "results.jsonl")
TIMEOUT = 600
# Tools that need arguments a sweep cannot give; each one is run by hand.
SKIP = {
    "wb_perf_487": "a measurement that needs two real repos as arguments",
    "wb_design_measure_diff": "compares two measure files given as arguments",
    "wb_closed_dialogs_946": "a measurement that needs `measure <variant>` or `patch`",
    "wb_detached_file": "needs the way to open the file: `menu`, `descriptor` or `tab`",
}


def kill_tree(pid):
    if os.name == "nt":
        subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    else:
        os.killpg(pid, 9)


def run_script(path, rel, name):
    log = os.path.join(LOGS, rel.split("/")[-2] + "__" + name + ".log")
    start = time.time()
    with tempfile.TemporaryFile() as tmp:
        kw = {"start_new_session": True} if os.name != "nt" else {
            "creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
        proc = subprocess.Popen([sys.executable, "-u", path], cwd=ROOT,
                                stdout=tmp, stderr=subprocess.STDOUT,
                                stdin=subprocess.DEVNULL, **kw)
        try:
            code = proc.wait(timeout=TIMEOUT)
            timed_out = False
        except subprocess.TimeoutExpired:
            kill_tree(proc.pid)
            code = proc.wait()
            timed_out = True
        tmp.seek(0)
        text = tmp.read().decode("utf-8", "replace")
    with open(log, "w", encoding="utf-8") as f:
        f.write(text)
    fails = [ln.strip() for ln in text.splitlines() if ln.lstrip().startswith("[FAIL]")]
    passes = sum(1 for ln in text.splitlines() if ln.lstrip().startswith("[PASS]"))
    tb = re.findall(r"^\w*(?:Error|Exception)\b.*$", text, re.M)
    return dict(
        status="timeout" if timed_out else ("pass" if code == 0 else "fail"),
        exit=code, seconds=round(time.time() - start),
        passes=passes, fails=fails[:20], error=tb[-1] if tb else None,
        tail=text.splitlines()[-6:],
    )


def main():
    args = sys.argv[1:]
    list_only = "--list" in args
    filters = [a for a in args if a != "--list"]
    scripts = sorted(glob.glob(os.path.join(ROOT, "tests", "browser", "*", "wb_*.py")))
    if filters:
        scripts = [s for s in scripts if any(a in s for a in filters)]
    if list_only:
        for path in scripts:
            rel = os.path.relpath(path, ROOT).replace("\\", "/")
            reason = SKIP.get(os.path.basename(path)[:-3])
            print(f"{rel}  skipped ({reason})" if reason else f"{rel}  run")
        return
    os.makedirs(LOGS, exist_ok=True)
    done = set()
    if os.path.exists(RESULTS):
        with open(RESULTS, encoding="utf-8") as f:
            done = {json.loads(line)["script"] for line in f if line.strip()}
    for i, path in enumerate(scripts, 1):
        rel = os.path.relpath(path, ROOT).replace("\\", "/")
        name = os.path.basename(path)[:-3]
        if rel in done:
            continue
        rec = {"script": rel}
        if name in SKIP:
            rec.update(status="skipped", reason=SKIP[name])
        else:
            rec.update(run_script(path, rel, name))
        with open(RESULTS, "a", encoding="utf-8") as f:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
        print(f"[{i}/{len(scripts)}] {rec['status']:8} {rel}", flush=True)


main()
