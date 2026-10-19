"""#746 browser acceptance: uploading files from the operator's computer.

One Playwright pass over a REAL daemon:

Scenario a  files dropped on a folder row open the dialog; Upload sends them,
            the card reaches Done, the bytes land in that folder on disk, and
            the file appears in the tree
Scenario b  a drop on a FILE row means its folder; a name that exists asks
            once; "Keep both" lands `a (1).txt` and leaves `a.txt` intact
Scenario c  "Replace" overwrites the existing file
Scenario d  the folder menu has "Upload…" (a file row does not); it opens the
            file picker, and the picked file is uploaded to that folder
Scenario e  a 25 MB file goes in parts; one request is dropped in the middle,
            the card stops with "Try again", and Try again finishes from the
            confirmed offset: the bytes on disk are the same as the source
Scenario f  a file over files.upload_max_mb is refused in the dialog
Scenario h  a drag that allows only `copy` (as from Explorer) resting on a
            closed folder marks the row as the target; a drag that only passes
            over it does not open it, and 1 s of rest does, and the drop still lands there
Scenario g  the audit log has one file.upload line per finished file, not one
            per part

Boots a Localhost daemon on 7446 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/746-upload.png.
Run: python tests/browser/files/wb_upload_746.py   (exit 0 = all pass)
"""

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7446
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SHOT = os.path.join(SHOT_DIR, "746-upload.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
FILES = "Alpine.$data(document.querySelector('[x-data=\"wbFiles\"]'))"
DIALOG = "Alpine.$data(document.querySelector('[x-data=\"wbUploadDialog\"]'))"
UPLOADS = "Alpine.store('uploads')"
MB = 1024 * 1024

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def wait_listening(base, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def empty_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wb746_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed():
    d = Path(tempfile.mkdtemp(prefix="wb746_repo_")) / "upload-fixture"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("original\n", encoding="utf-8")
    (d / "docs").mkdir()
    (d / "docs" / "readme.md").write_text("# docs\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb746@example.com")
    git(d, "config", "user.name", "wb746")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", str(fixture_dir)], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are embedded in the binary: rebuild after any assets/ui edit.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# Drop `files` (name -> text) on the row whose rel path is `rel`, with a real
# DataTransfer and the three events a browser fires.
DROP = """
([rel, files]) => {
  const c = Alpine.$data(document.querySelector('[x-data="wbFiles"]'));
  // By rel path, not by node: `c._tree` is Alpine's Proxy, so its nodes are
  // not the objects `getNode` answers.
  const row = [...document.querySelectorAll('.wb-host .wb-row')].find(r => {
    const n = mar10.Wunderbaum.getNode(r);
    return n && c.relPath(n) === rel;
  });
  if (!row) return false;
  const dt = new DataTransfer();
  for (const [name, text] of Object.entries(files)) dt.items.add(new File([text], name));
  const box = row.getBoundingClientRect();
  const at = { bubbles: true, cancelable: true, dataTransfer: dt, clientX: box.left + 30, clientY: box.top + box.height / 2 };
  const target = row.querySelector('.wb-title') || row;
  for (const type of ['dragenter', 'dragover', 'drop']) target.dispatchEvent(new DragEvent(type, at));
  return true;
}
"""

# Rest a drag that allows only `copy` on the row of `rel` for `ms`, firing
# `dragover` as a browser does, and say what the row showed and the tree did.
HOVER = """
async ([rel, ms, mode]) => {
  // `mode`: 'pass' sweeps the pointer and leaves, 'rest' stays and leaves,
  // 'drop' stays and drops.
  const move = mode === 'pass';
  const c = Alpine.$data(document.querySelector('[x-data="wbFiles"]'));
  // The row is drawn again when the folder opens: each step finds the live
  // row, as the pointer of a real drag is always over the live element.
  const rowOf = () => [...document.querySelectorAll('.wb-host .wb-row')].find(r => {
    const n = mar10.Wunderbaum.getNode(r);
    return n && c.relPath(n) === rel;
  });
  const dt = new DataTransfer();
  dt.items.add(new File(['hovered'], 'hovered.txt'));
  dt.effectAllowed = 'copy';
  let step = 0;
  const fire = (type) => {
    const row = rowOf();
    const dx = move ? (step++ % 2) * 40 : 0;
    const target = row.querySelector('.wb-title') || row;
    const box = row.getBoundingClientRect();
    const ev = new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt, clientX: box.left + 30 + dx, clientY: box.top + box.height / 2 });
    target.dispatchEvent(ev);
    return { row, ev };
  };
  fire('dragenter');
  let marked = false;
  let accepted = false;
  let outline = '';
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const { row, ev } = fire('dragover');
    accepted = accepted || ev.defaultPrevented;
    if (row.classList.contains('wb-drop-over')) {
      marked = true;
      outline = getComputedStyle(row).outlineStyle;
    }
    await new Promise(r => setTimeout(r, 100));
  }
  const node = c._tree.findFirst(x => c.relPath(x) === rel);
  if (mode !== 'drop') {
    fire('dragleave');
    return { expanded: !!node?.expanded };
  }
  fire('drop');
  return { marked, accepted, expanded: !!node?.expanded, outline };
}
"""

DIALOG_STATE = f"""
() => {{
  const d = {DIALOG};
  const modal = document.querySelector('.upload-picker');
  return {{
    open: d.upload.open,
    laid: !!modal && modal.offsetParent !== null && modal.clientWidth > 0,
    busy: d.upload.busy,
    folder: d.upload.folder,
    rows: d.upload.rows.map(r => [r.name, r.tooBig, r.exists]),
    existing: d.uploadHasExisting(),
    blocked: d.uploadBlocked(),
    error: d.upload.error,
  }};
}}
"""

ITEMS = f"() => {UPLOADS}.items.map(i => ({{ name: i.name, state: i.state, path: i.path, sent: i.sent, error: i.error }}))"


def drop(page, rel, files):
    """Drop once the row of `rel` is drawn: Wunderbaum draws rows after the
    model changes, so a row of a folder just opened is not there at once."""
    page.wait_for_function(
        f"(rel) => {{ const c = {FILES}; return [...document.querySelectorAll('.wb-host .wb-row')]"
        "  .some(r => { const n = mar10.Wunderbaum.getNode(r); return n && c.relPath(n) === rel; }); }",
        arg=rel,
        timeout=15000,
    )
    return page.evaluate(DROP, [rel, files])


def wait_dialog(page):
    page.wait_for_function(f"() => {DIALOG}.upload.open === true && {DIALOG}.upload.busy === false", timeout=10000)
    page.wait_for_function(
        "() => { const m = document.querySelector('.upload-picker'); return !!m && m.offsetParent !== null && m.clientWidth > 0; }",
        timeout=10000,
    )
    return page.evaluate(DIALOG_STATE)


def click_upload(page):
    page.evaluate(
        "() => [...document.querySelectorAll('.upload-picker .modal-foot .btn')].find(b => b.textContent.trim() === 'Upload').click()"
    )


def wait_all_done(page, count, timeout=60000):
    page.wait_for_function(
        f"(n) => {UPLOADS}.items.length === n && {UPLOADS}.items.every(i => i.state === 'done')",
        arg=count,
        timeout=timeout,
    )


def tree_has(page, rel, timeout=15000):
    try:
        page.wait_for_function(
            f"(rel) => {{ const c = {FILES}; return !!c._tree.findFirst(x => c.relPath(x) === rel); }}",
            arg=rel,
            timeout=timeout,
        )
        return True
    except Exception:
        return False


def menu_labels(page, rel):
    page.evaluate(
        f"(rel) => {{ const c = {FILES}; const n = rel ? c._tree.findFirst(x => c.relPath(x) === rel) : null;"
        "  if (n) n.setActive(); c.showMenu(120, 120, n); }",
        arg=rel,
    )
    labels = page.evaluate(
        "() => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
        "  .filter(b => b.offsetParent !== null).map(b => b.querySelector('span')?.textContent.trim())"
    )
    page.keyboard.press("Escape")
    return labels


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb746_reg_")
    fixture = seed()
    slug = register_fixture(daemon_dir, fixture)
    big = Path(tempfile.mkdtemp(prefix="wb746_big_")) / "big.bin"
    big.write_bytes(os.urandom(25 * MB))
    over = Path(tempfile.mkdtemp(prefix="wb746_over_")) / "over.bin"

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function("(s) => Alpine.store('projects').openSlug === s", arg=slug, timeout=15000)
            check("the tree shows a.txt", tree_has(page, "a.txt"))

            # --- a: drop on a folder ---------------------------------------
            ok = drop(page, "docs", {"one.txt": "first file\n", "two.txt": "second\n"})
            check("a: the drop reached a folder row", ok)
            st = wait_dialog(page)
            check("a: the dialog opens for docs", st["folder"] == "docs", str(st))
            check(
                "a: it lists both files, none over the limit, none existing",
                st["rows"] == [["one.txt", False, False], ["two.txt", False, False]] and not st["existing"],
                str(st["rows"]),
            )
            click_upload(page)
            wait_all_done(page, 2)
            check("a: one.txt is on disk", (fixture / "docs" / "one.txt").read_text(encoding="utf-8") == "first file\n")
            check("a: two.txt is on disk", (fixture / "docs" / "two.txt").read_text(encoding="utf-8") == "second\n")
            card = page.evaluate(
                "() => { const c = document.querySelector('.upload-card'); return !!c && c.offsetParent !== null && c.clientWidth > 0; }"
            )
            check("a: the progress card is laid out", card)
            page.screenshot(path=SHOT)
            page.evaluate(f"() => {FILES}._tree.findFirst(x => x.title === 'docs').setExpanded(true)")
            check("a: docs/one.txt appears in the tree", tree_has(page, "docs/one.txt"))
            page.evaluate(f"() => {UPLOADS}.clearFinished()")

            # --- b: drop on a file, keep both ----------------------------------
            check("b: docs/readme.md is in the tree", tree_has(page, "docs/readme.md"))
            ok = drop(page, "docs/readme.md", {"three.txt": "3"})
            check("b: the drop reached a file row", ok)
            st = wait_dialog(page)
            check("b: a drop on a file means its folder", st["folder"] == "docs", str(st))
            page.evaluate(f"() => {DIALOG}.uploadCancel()")
            drop(page, "a.txt", {"a.txt": "dropped\n"})
            st = wait_dialog(page)
            check("b: a.txt is marked as existing, at the root", st["folder"] == "" and st["existing"], str(st))
            check(
                "b: the choice is laid out",
                page.evaluate("() => { const f = document.querySelector('.upload-choice'); return !!f && f.offsetParent !== null; }"),
            )
            page.check(".upload-choice input[value='keep_both']")
            click_upload(page)
            wait_all_done(page, 1)
            items = page.evaluate(ITEMS)
            check("b: keep both lands a (1).txt", items[0]["path"] == "a (1).txt", str(items))
            check("b: a (1).txt has the dropped bytes", (fixture / "a (1).txt").read_text(encoding="utf-8") == "dropped\n")
            check("b: a.txt is intact", (fixture / "a.txt").read_text(encoding="utf-8") == "original\n")
            page.evaluate(f"() => {UPLOADS}.clearFinished()")

            # --- c: replace -----------------------------------------------------
            drop(page, "a.txt", {"a.txt": "replaced\n"})
            wait_dialog(page)
            page.check(".upload-choice input[value='replace']")
            click_upload(page)
            wait_all_done(page, 1)
            check("c: replace overwrites a.txt", (fixture / "a.txt").read_text(encoding="utf-8") == "replaced\n")
            page.evaluate(f"() => {UPLOADS}.clearFinished()")

            # --- d: Upload… in the folder menu ---------------------------------
            check("d: a folder row has Upload…", "Upload…" in menu_labels(page, "docs"))
            check("d: the root has Upload…", "Upload…" in menu_labels(page, ""))
            check("d: a file row has no Upload…", "Upload…" not in menu_labels(page, "a.txt"))
            picked = Path(tempfile.mkdtemp(prefix="wb746_pick_")) / "picked.txt"
            picked.write_text("picked\n", encoding="utf-8")
            page.evaluate(
                f"(rel) => {{ const c = {FILES}; const n = c._tree.findFirst(x => c.relPath(x) === rel); n.setActive(); c.showMenu(120, 120, n); }}",
                arg="docs",
            )
            with page.expect_file_chooser() as fc:
                page.evaluate(
                    "() => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
                    "  .find(b => b.querySelector('span')?.textContent.trim() === 'Upload…').click()"
                )
            fc.value.set_files(str(picked))
            st = wait_dialog(page)
            check("d: the picked file opens the dialog for docs", st["folder"] == "docs" and st["rows"][0][0] == "picked.txt", str(st))
            click_upload(page)
            wait_all_done(page, 1)
            check("d: picked.txt is on disk", (fixture / "docs" / "picked.txt").read_text(encoding="utf-8") == "picked\n")
            page.evaluate(f"() => {UPLOADS}.clearFinished()")

            # --- e: 25 MB with one dropped request -------------------------------
            parts = {"n": 0, "dropped": False}

            def route(r):
                body = r.request.post_data or ""
                if '"file.upload"' in body:
                    parts["n"] += 1
                    if parts["n"] == 6 and not parts["dropped"]:
                        parts["dropped"] = True
                        return r.abort()
                return r.continue_()

            page.route("**/api/command", route)
            page.evaluate(
                f"(rel) => {{ const c = {FILES}; const n = c._tree.findFirst(x => c.relPath(x) === rel); n.setActive(); c.showMenu(120, 120, n); }}",
                arg="docs",
            )
            with page.expect_file_chooser() as fc:
                page.evaluate(
                    "() => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
                    "  .find(b => b.querySelector('span')?.textContent.trim() === 'Upload…').click()"
                )
            fc.value.set_files(str(big))
            st = wait_dialog(page)
            check("e: 25 MB is not over the default limit", st["rows"] == [["big.bin", False, False]], str(st))
            click_upload(page)
            page.wait_for_function(f"() => {UPLOADS}.items[0]?.state === 'failed'", timeout=60000)
            items = page.evaluate(ITEMS)
            check("e: the dropped request stops the file", items[0]["state"] == "failed", str(items))
            check("e: the daemon confirmed 5 parts before it", items[0]["sent"] == 5 * MB, str(items[0]["sent"]))
            retry = page.evaluate(
                "() => { const b = [...document.querySelectorAll('.upload-card .btn')].find(b => b.textContent.trim() === 'Try again');"
                "  return !!b && b.offsetParent !== null; }"
            )
            check("e: the card offers Try again", retry)
            page.evaluate(
                "() => [...document.querySelectorAll('.upload-card .btn')].find(b => b.textContent.trim() === 'Try again').click()"
            )
            wait_all_done(page, 1, timeout=120000)
            page.unroute("**/api/command")
            same = hashlib.sha256((fixture / "docs" / "big.bin").read_bytes()).hexdigest() == hashlib.sha256(big.read_bytes()).hexdigest()
            check("e: the 25 MB file on disk is the same as the source", same)
            check("e: 25 parts and the dropped one were sent", parts["n"] == 26, str(parts))
            check("e: no part file is left", not any((fixture / ".ralphy" / "uploads").glob("*.part")))
            page.evaluate(f"() => {UPLOADS}.clearFinished()")

            # --- f: over the limit -----------------------------------------------
            subprocess.run([EXE, "config", "set", "files.upload_max_mb", "1"], cwd=fixture, check=True, capture_output=True)
            over.write_bytes(b"x" * (MB + 1))
            page.evaluate(
                f"(rel) => {{ const c = {FILES}; const n = c._tree.findFirst(x => c.relPath(x) === rel); n.setActive(); c.showMenu(120, 120, n); }}",
                arg="docs",
            )
            with page.expect_file_chooser() as fc:
                page.evaluate(
                    "() => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
                    "  .find(b => b.querySelector('span')?.textContent.trim() === 'Upload…').click()"
                )
            fc.value.set_files(str(over))
            st = wait_dialog(page)
            check("f: a file over the 1 MB limit is refused", st["rows"] == [["over.bin", True, False]], str(st))
            check("f: Upload is blocked", st["blocked"])
            page.evaluate(f"() => {DIALOG}.uploadCancel()")

            # --- h: hover marks the target and opens a closed folder -------------
            (fixture / "deep").mkdir()
            (fixture / "deep" / "x.txt").write_text("x", encoding="utf-8")
            check("h: the tree shows the new closed folder", tree_has(page, "deep"))
            page.wait_for_function(
                f"() => {{ const c = {FILES}; return [...document.querySelectorAll('.wb-host .wb-row')]"
                "  .some(r => { const n = mar10.Wunderbaum.getNode(r); return n && c.relPath(n) === 'deep'; }); }",
                timeout=15000,
            )
            passing = page.evaluate(HOVER, ["deep", 2000, "pass"])
            check("h: a drag that keeps moving over the folder does not open it", not passing["expanded"], str(passing))
            h = page.evaluate(HOVER, ["deep", 700, "rest"])
            check("h: 0.7 s of rest does not open it yet", not h["expanded"], str(h))
            h = page.evaluate(HOVER, ["deep", 1500, "drop"])
            check("h: the row takes a copy-only drag", h["accepted"], str(h))
            check("h: the row is marked as the drop target", h["marked"] and h["outline"] == "dashed", str(h))
            check("h: the closed folder opens while the drag rests on it", h["expanded"], str(h))
            st = wait_dialog(page)
            check("h: the drop opens the dialog for the folder", st["folder"] == "deep", str(st))
            page.evaluate(f"() => {DIALOG}.uploadCancel()")

            check("no page error", not thrown, str(thrown))
            browser.close()

        # --- g: the audit log ---------------------------------------------------
        lines = [json.loads(l) for l in Path(daemon_dir, "daemon-audit.jsonl").read_text(encoding="utf-8").splitlines() if l.strip()]
        uploads = [l for l in lines if l.get("verb") == "file.upload"]
        check(
            "g: one audit line per finished file",
            sorted((u.get("file"), u.get("result")) for u in uploads)
            == sorted([
                ("docs/one.txt", "created"),
                ("docs/two.txt", "created"),
                ("a (1).txt", "created"),
                ("a.txt", "replaced"),
                ("docs/picked.txt", "created"),
                ("docs/big.bin", "created"),
            ]),
            str([(u.get("file"), u.get("result"), u.get("size")) for u in uploads]),
        )
        big_line = [u for u in uploads if u.get("file") == "docs/big.bin"]
        check("g: the audit line has the size", bool(big_line) and big_line[0].get("size") == 25 * MB)
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} passed")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
