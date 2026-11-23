"""Browser check of three file viewers (ADR-0091 D9, D11, D12), in Chromium.

Over a REAL daemon with a scratch store:

  1. a PDF pane shows the loading note, then drops it once the pages draw;
  2. its zoom bar stays in place while the pages scroll;
  3. the page field goes to a typed page;
  4. an xlsx grid draws without the Tabulator `formatter` warning;
  5. an audio pane's controller is in audio mode and its bar has a height.

The fixtures are written by this script. The daemon is stopped by its own
subprocess handle, never by name.

Run: python tests/browser/files/wb_viewer_wait_and_controls.py   (exit 0 = all pass)
"""

import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
SHOTS = Path(REPO_ROOT) / ".ralphy" / "screenshots"
PAGES = 12

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def pdf_bytes(pages):
    """A text PDF of `pages` pages, with a correct cross-reference table."""
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>"]
    kids = " ".join(f"{3 + 2 * i} 0 R" for i in range(pages))
    objs.append(f"<< /Type /Pages /Kids [{kids}] /Count {pages} >>".encode())
    font = 3 + 2 * pages
    for i in range(pages):
        stream = f"BT /F1 36 Tf 72 700 Td (Page {i + 1}) Tj ET".encode()
        objs.append(
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {4 + 2 * i} 0 R "
            f"/Resources << /Font << /F1 {font} 0 R >> >> >>".encode()
        )
        objs.append(b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream")
    objs.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for n, body in enumerate(objs, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % n + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
    for off in offsets:
        out += b"%010d 00000 n \n" % off
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
    return bytes(out)


def xlsx_bytes(path):
    """A one-sheet xlsx with inline strings, written by hand."""
    ns = "http://schemas.openxmlformats.org/"
    rows = "".join(
        f'<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>name {r}</t></is></c>'
        f'<c r="B{r}" t="inlineStr"><is><t>&lt;b&gt;{r}&lt;/b&gt;</t></is></c></row>'
        for r in range(1, 6)
    )
    parts = {
        "[Content_Types].xml": (
            f'<Types xmlns="{ns}package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" ContentType="application/'
            'vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/'
            'vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>'
        ),
        "_rels/.rels": (
            f'<Relationships xmlns="{ns}package/2006/relationships">'
            f'<Relationship Id="r1" Type="{ns}officeDocument/2006/relationships/officeDocument" '
            'Target="xl/workbook.xml"/></Relationships>'
        ),
        "xl/workbook.xml": (
            f'<workbook xmlns="{ns}spreadsheetml/2006/main" '
            f'xmlns:r="{ns}officeDocument/2006/relationships">'
            '<sheets><sheet name="Data" sheetId="1" r:id="r1"/></sheets></workbook>'
        ),
        "xl/_rels/workbook.xml.rels": (
            f'<Relationships xmlns="{ns}package/2006/relationships">'
            f'<Relationship Id="r1" Type="{ns}officeDocument/2006/relationships/worksheet" '
            'Target="worksheets/sheet1.xml"/></Relationships>'
        ),
        "xl/worksheets/sheet1.xml": (
            f'<worksheet xmlns="{ns}spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>'
        ),
    }
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts.items():
            z.writestr(name, text)


def wav_bytes(seconds=2, rate=8000):
    data = b"\x80" * (seconds * rate)
    fmt = struct.pack("<HHIIHH", 1, 1, rate, rate, 1, 8)
    body = b"WAVE" + b"fmt " + struct.pack("<I", 16) + fmt + b"data" + struct.pack("<I", len(data)) + data
    return b"RIFF" + struct.pack("<I", len(body)) + body


def make_fixture_repo():
    d = Path(tempfile.mkdtemp(prefix="wbviewers_repo_"))
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "long.pdf").write_bytes(pdf_bytes(PAGES))
    xlsx_bytes(d / "book.xlsx")
    (d / "tone.wav").write_bytes(wav_bytes())
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbviewers@example.com"],
        ["git", "config", "user.name", "wbviewers"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return str(d)


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    out = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    return out.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def wait_listening(base, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def launch(daemon_dir, port):
    empty = tempfile.mkdtemp(prefix="wbviewers_empty_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(port)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def open_path(page, slug, path):
    page.evaluate(
        "([s, p]) => Alpine.store('tabs').openLink({ project: s, path: p, checkout: null })", [slug, path]
    )


def pane(slug, path):
    return f'.viewer[data-tab-id="file:{slug}:{path}"]'


def check_pdf(page, slug):
    sel = pane(slug, "long.pdf")
    # Hold the bytes route so the wait is long enough to see.
    page.route("**/api/file/bytes?*long.pdf*", lambda route: (time.sleep(1.5), route.continue_()))
    open_path(page, slug, "long.pdf")
    page.wait_for_selector(f"{sel} .viewer-loading", timeout=10000)
    check("pdf: the loading note shows while the file loads", True)
    page.wait_for_selector(f"{sel} .pdf-pages .page", timeout=30000)
    page.wait_for_function(f"() => !document.querySelector('{sel} .viewer-loading')", timeout=10000)
    check("pdf: the loading note goes once the pages draw", True)
    bar = f"{sel} > .zoom-bar"
    before = page.eval_on_selector(bar, "el => el.getBoundingClientRect().top")
    page.eval_on_selector(f"{sel} .pdf-body", "el => { el.scrollTop = el.scrollHeight / 2; }")
    page.wait_for_timeout(300)
    after = page.eval_on_selector(bar, "el => el.getBoundingClientRect().top")
    check("pdf: the zoom bar stays in place while the pages scroll", before == after, f"{before} -> {after}")
    page.eval_on_selector(f"{sel} .pdf-body", "el => { el.scrollTop = 0; }")
    field = f"{sel} .pdf-page"
    page.fill(field, "9")
    page.press(field, "Enter")
    page.wait_for_timeout(500)
    top = page.eval_on_selector(
        f"{sel}",
        "el => { const p = el.querySelector('.page[data-page-number=\"9\"]');"
        " return p.getBoundingClientRect().top - el.querySelector('.pdf-body').getBoundingClientRect().top; }",
    )
    check("pdf: the page field goes to page 9", abs(top) < 50, f"offset={top}")
    page.screenshot(path=str(SHOTS / "wb_viewer_pdf.png"))


def check_sheet(page, slug, warnings):
    open_path(page, slug, "book.xlsx")
    sel = pane(slug, "book.xlsx")
    page.wait_for_selector(f"{sel} .tabulator-cell", timeout=30000)
    cells = page.eval_on_selector_all(f"{sel} .tabulator-cell", "els => els.map(e => e.innerHTML)")
    check("sheet: a cell with markup text draws as text", "&lt;b&gt;1&lt;/b&gt;" in cells, f"{cells[:4]}")
    bad = [w for w in warnings if "formatter" in w]
    check("sheet: no Tabulator formatter warning", not bad, f"{bad[:1]}")
    page.screenshot(path=str(SHOTS / "wb_viewer_sheet.png"))


def check_audio(page, slug):
    open_path(page, slug, "tone.wav")
    sel = pane(slug, "tone.wav")
    page.wait_for_selector(f"{sel} media-controller[audio]", timeout=15000)
    page.wait_for_timeout(3500)
    height = page.eval_on_selector(f"{sel} media-control-bar", "el => el.getBoundingClientRect().height")
    check("audio: the controller is in audio mode and its bar shows", height > 0, f"height={height}")
    page.screenshot(path=str(SHOTS / "wb_viewer_audio.png"))


def main():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") != "1":
        subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    SHOTS.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}/"
    daemon_dir = tempfile.mkdtemp(prefix="wbviewers_reg_")
    slug = register_fixture(daemon_dir, make_fixture_repo())
    proc = launch(daemon_dir, port)
    try:
        if not wait_listening(base):
            check(f"daemon listening on {port}", False)
            sys.exit(1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True)
            page = browser.new_context(viewport={"width": 1400, "height": 900}).new_page()
            warnings = []
            page.on("console", lambda m: warnings.append(m.text))
            page.goto(base)
            page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
            page.evaluate(f"(s) => {{ if (Alpine.store('projects').openSlug !== s) {SH}.toggle(s); }}", slug)
            check_pdf(page, slug)
            check_sheet(page, slug, warnings)
            check_audio(page, slug)
            browser.close()
    finally:
        stop(proc)
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if len(results) == 7 and all(results) else 1)


if __name__ == "__main__":
    main()
