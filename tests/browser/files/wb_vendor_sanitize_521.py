"""#521/#522 browser acceptance: the vendored DOMPurify and mermaid, after an
update, still render diagrams and still remove an event handler.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7521. The fixture and
the note helpers are `wb_note_on_top.py`'s.

The same text is a Markdown file and a note: a raw `<img onerror>`, a
flowchart and a sequence diagram.

P1  the Markdown preview draws both diagrams as SVG, with their labels
P2  …the `<img>` is drawn without its `onerror`, and the handler never ran
N1  the note card draws both diagrams as SVG, with their labels
N2  …the card holds no element with an `onerror` (it shows raw HTML as text),
    and the handler never ran
E   no page errors were thrown

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/files/wb_vendor_sanitize_521.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "notes"))
import wb_note_on_top as N  # noqa: E402  the fixture and note helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

T = N.T
PORT = 7521
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SH = T.SH
SHOT_DIR = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots")
VIEW = {"width": 1600, "height": 1000}

TEXT = """# Diagrams

<img src="x" onerror="window.__xss = 1">

```mermaid
flowchart TD
  Start --> Finish
```

```mermaid
sequenceDiagram
  Alice->>Bob: Hello
```
"""

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def fixture():
    fx = T.make_fixture_repo()
    # A note is a container, not plain text: the page writes it with the
    # card's own `note.write`.
    Path(fx, "DIAGRAMS.md").write_text(TEXT, encoding="utf-8")
    for args in (["git", "add", "-A"], ["git", "commit", "-m", "diagrams"]):
        subprocess.run(args, cwd=fx, check=True, capture_output=True)
    return fx


# What a container drew: its diagrams' SVGs, the text of their labels (the
# SVG's own <style> left out), and whether any element in it still carries an
# `onerror`.
DRAWN = r"""(sel) => { const root = document.querySelector(sel);
  if (!root) return null;
  const svgs = Array.from(root.querySelectorAll('svg')).filter(s => s.closest('.mermaid, .note-mermaid'));
  const label = (s) => Array.from(s.querySelectorAll('text, foreignObject'))
    .map(t => t.textContent.trim()).filter(Boolean).join(' ');
  return { svgs: svgs.length,
           labels: svgs.map(label),
           imgs: root.querySelectorAll('img[src="x"]').length,
           onerror: root.querySelectorAll('[onerror]').length,
           errors: root.querySelectorAll('.mermaid-error, .note-mermaid-error').length }; }"""


def labelled(drawn):
    if not drawn or drawn["svgs"] != 2:
        return False
    flow, seq = drawn["labels"]
    return "Start" in flow and "Finish" in flow and "Alice" in seq and "Bob" in seq and "Hello" in seq


def clean(page, drawn):
    return (
        drawn is not None
        and drawn["onerror"] == 0
        and drawn["errors"] == 0
        and page.evaluate("() => window.__xss === undefined")
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    d = tempfile.mkdtemp(prefix="wb521_")
    fx = fixture()
    slug = T.register_fixture(d, fx)
    N.write_desk(d, slug)
    proc = T.launch(d)
    errors = []
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            b = p.chromium.launch()
            page = b.new_context(viewport=dict(VIEW)).new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            N.boot(page, slug)

            # --- P: the Markdown preview -------------------------------------
            page.evaluate(
                "([project, path]) => " f"{SH}.openTab({{ project, path, title: path, ftype: 'markdown' }})",
                [slug, "DIAGRAMS.md"],
            )
            try:
                page.wait_for_function("() => document.querySelectorAll('.mermaid svg').length >= 2", timeout=30000)
            except Exception:
                pass
            preview = page.evaluate(DRAWN, "body")
            check("P1 the preview draws both diagrams with their labels", labelled(preview), str(preview))
            # The preview keeps the <img> and drops only its handler.
            check(
                "P2 …the image is drawn without its onerror, and the handler never ran",
                clean(page, preview) and preview["imgs"] == 1,
                str(preview),
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "521-522-markdown-preview.png"))

            # --- N: the note card --------------------------------------------
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            wrote = page.evaluate(
                """([repo, path, markdown]) => WBDaemon.write('note.write', { repo, path, markdown })""",
                [slug, "diagrams.note", "---\ncolor: sand\n---\n" + TEXT],
            )
            if not isinstance(wrote, dict) or wrote.get("status") == "error":
                print(f"[INFO] note.write answered {wrote}", flush=True)
            page.evaluate(
                """([repo, path]) => { const ws = document.getElementById('workspace');
                  WBNotes.openFromExplorer({ repo, path,
                    viewport: { width: ws.clientWidth, height: ws.clientHeight },
                    offset: { left: ws.scrollLeft, top: ws.scrollTop } }); }""",
                [slug, "diagrams.note"],
            )
            nid = page.wait_for_function(
                "(path) => WBConsole.notes().find((n) => n.path === path)?.id",
                arg="diagrams.note",
                timeout=10000,
            ).json_value()
            card = f".note-card[data-note-id='{nid}']"
            try:
                page.wait_for_function(
                    "(sel) => document.querySelectorAll(sel + ' .note-mermaid svg').length >= 2",
                    arg=card,
                    timeout=30000,
                )
            except Exception:
                pass
            note = page.evaluate(DRAWN, card)
            check("N1 the note draws both diagrams with their labels", labelled(note), str(note))
            # The note shows raw HTML as text, so no <img> is drawn at all.
            check("N2 …no element keeps an onerror, and the handler never ran", clean(page, note), str(note))
            if note is not None:
                page.locator(card).screenshot(path=os.path.join(SHOT_DIR, "521-522-note.png"))

            check("E no page errors were thrown", not errors, str(errors))
            b.close()
    finally:
        T.stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # Pinned exactly: a scenario that silently stops running must not pass.
    if len(results) != 5:
        print(f"[FAIL] expected 5 checks, ran {len(results)}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
