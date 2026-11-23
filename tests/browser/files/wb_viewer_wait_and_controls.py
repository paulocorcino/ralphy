"""Browser check of three file viewers (ADR-0091 D9, D11, D12), in Chromium.

Over a REAL daemon with a scratch store:

  1. a PDF pane shows the loading note, then drops it once the pages draw;
  2. its zoom bar stays in place while the pages scroll;
  3. the page field goes to a typed page;
  4. an xlsx grid draws without a Tabulator column warning, shows every
     column, does not cut its text, numbers its rows, and hides the sheet
     selector for a file of one sheet;
  5. an audio pane's controller is in audio mode and its bar has a height;
  6. a docx draws with its layout in a shadow root: bold text, justified
     text and the header image, with no error in the console;
  7. a zip lists its entries as a table that scrolls, with a header that
     stays, names indented under their folder, and sizes in their column;
  8. a semicolon csv opens as a grid split on semicolons, and the separator
     selector changes the split;
  9. an odt draws on a white page: its font, bold, highlight, an image, and
     a list that continues the numbering of the list before it.

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
import zlib
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


def png_bytes(width=40, height=20):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes((30, 140, 60)) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    signature = b"\x89PNG\r\n\x1a\n"
    return signature + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def docx_bytes(path):
    """A docx with a bold title, a justified paragraph and an image in the header."""
    w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
    r = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    pkg = "http://schemas.openxmlformats.org/package/2006/relationships"
    wp = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
    a = "http://schemas.openxmlformats.org/drawingml/2006/main"
    pic = "http://schemas.openxmlformats.org/drawingml/2006/picture"
    drawing = (
        f'<w:drawing><wp:inline><wp:extent cx="762000" cy="381000"/><wp:docPr id="1" name="logo"/>'
        f'<a:graphic xmlns:a="{a}"><a:graphicData uri="{pic}"><pic:pic xmlns:pic="{pic}">'
        '<pic:nvPicPr><pic:cNvPr id="1" name="logo.png"/><pic:cNvPicPr/></pic:nvPicPr>'
        '<pic:blipFill><a:blip r:embed="img1"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
        '<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="762000" cy="381000"/></a:xfrm>'
        '<a:prstGeom prst="rect"/></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing>'
    )
    parts = {
        "[Content_Types].xml": (
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Default Extension="png" ContentType="image/png"/>'
            '<Override PartName="/word/document.xml" ContentType="application/'
            'vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
            '<Override PartName="/word/header1.xml" ContentType="application/'
            'vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/></Types>'
        ),
        "_rels/.rels": (
            f'<Relationships xmlns="{pkg}"><Relationship Id="r1" '
            f'Type="{r}/officeDocument" Target="word/document.xml"/></Relationships>'
        ),
        "word/_rels/document.xml.rels": (
            f'<Relationships xmlns="{pkg}"><Relationship Id="h1" Type="{r}/header" '
            'Target="header1.xml"/></Relationships>'
        ),
        "word/_rels/header1.xml.rels": (
            f'<Relationships xmlns="{pkg}"><Relationship Id="img1" Type="{r}/image" '
            'Target="media/logo.png"/></Relationships>'
        ),
        "word/header1.xml": (
            f'<w:hdr xmlns:w="{w}" xmlns:r="{r}" xmlns:wp="{wp}"><w:p><w:r>{drawing}</w:r></w:p></w:hdr>'
        ),
        "word/document.xml": (
            f'<w:document xmlns:w="{w}" xmlns:r="{r}"><w:body>'
            '<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>CONTRACT TITLE</w:t></w:r></w:p>'
            '<w:p><w:pPr><w:jc w:val="both"/></w:pPr><w:r><w:t xml:space="preserve">'
            + "The parties agree to the terms below. " * 12
            + '</w:t></w:r></w:p><w:sectPr><w:headerReference w:type="default" r:id="h1"/>'
            '<w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1701" w:bottom="1417" '
            'w:left="1701" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>'
        ),
    }
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in parts.items():
            z.writestr(name, text)
        z.writestr("word/media/logo.png", png_bytes())


def odt_bytes(path):
    """An odt with a font face, bold, a highlight, two lists and an image, from fixtures/odt."""
    folder = Path(__file__).parent / "fixtures" / "odt"
    with zipfile.ZipFile(path, "w") as z:
        z.writestr(zipfile.ZipInfo("mimetype"), "application/vnd.oasis.opendocument.text")
        for name in ("content.xml", "styles.xml"):
            z.writestr(name, (folder / name).read_text(encoding="utf-8"), zipfile.ZIP_DEFLATED)
        z.writestr("Pictures/logo.png", png_bytes())


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
    docx_bytes(d / "contract.docx")
    odt_bytes(d / "notes.odt")
    with zipfile.ZipFile(d / "bundle.zip", "w") as z:
        for i in range(80):
            z.writestr(f"top/inner/file{i:02d}.txt", "x" * (1000 + i))
    (d / "money.csv").write_text("name;value\nfig;3,5\nplum;1,25\n", encoding="utf-8")
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
    bad = [w for w in warnings if "Invalid column definition" in w or "formatter" in w]
    check("sheet: no Tabulator column warning", not bad, f"{bad[:1]}")
    got = page.evaluate(
        f"""() => {{
        const pane = document.querySelector('{sel}');
        const box = pane.querySelector('.sheet-body').getBoundingClientRect();
        const colB = [...pane.querySelectorAll('.tabulator-col')].find(c => c.textContent.trim() === 'B');
        const b = colB ? colB.getBoundingClientRect() : null;
        const cell = pane.querySelector('.tabulator-row .tabulator-cell[tabulator-field="c0"]');
        const nums = [...pane.querySelectorAll('.tabulator-row .sheet-rownum')].map(c => c.textContent);
        return {{
          bShown: !!b && b.width > 20 && b.right <= box.right,
          clipped: cell ? cell.scrollHeight > cell.clientHeight + 1 : null,
          nums: nums.slice(0, 3),
          selectHidden: pane.querySelector('.sheet-select').hidden,
        }};
      }}"""
    )
    check("sheet: column B is shown inside the pane", got["bShown"], f"{got}")
    check("sheet: a cell's text is not cut", got["clipped"] is False, f"{got['clipped']}")
    check("sheet: the rows are numbered", got["nums"] == ["1", "2", "3"], f"{got['nums']}")
    check("sheet: one sheet shows no sheet selector", got["selectHidden"], "")
    page.screenshot(path=str(SHOTS / "wb_viewer_sheet.png"))


def check_audio(page, slug):
    open_path(page, slug, "tone.wav")
    sel = pane(slug, "tone.wav")
    page.wait_for_selector(f"{sel} media-controller[audio]", timeout=15000)
    page.wait_for_timeout(3500)
    height = page.eval_on_selector(f"{sel} media-control-bar", "el => el.getBoundingClientRect().height")
    check("audio: the controller is in audio mode and its bar shows", height > 0, f"height={height}")
    page.screenshot(path=str(SHOTS / "wb_viewer_audio.png"))


def check_docx(page, slug, warnings):
    open_path(page, slug, "contract.docx")
    sel = pane(slug, "contract.docx")
    page.wait_for_selector(f"{sel} .docx-page", timeout=30000)
    page.wait_for_function(
        f"() => !!document.querySelector('{sel} .docx-page').shadowRoot.querySelector('section')",
        timeout=15000,
    )
    got = page.evaluate(
        f"""() => {{
        const root = document.querySelector('{sel} .docx-page').shadowRoot;
        const bold = [...root.querySelectorAll('span')].find(s => s.textContent === 'CONTRACT TITLE');
        const para = [...root.querySelectorAll('p')].find(p => p.textContent.startsWith('The parties'));
        const img = root.querySelector('header img');
        return {{
          weight: bold ? getComputedStyle(bold).fontWeight : null,
          align: para ? getComputedStyle(para).textAlign : null,
          img: img ? {{ src: img.getAttribute('src').slice(0, 5), w: img.naturalWidth }} : null,
          leaked: getComputedStyle(document.body).fontFamily,
        }};
      }}"""
    )
    check("docx: the bold title is bold", got["weight"] == "700", f"{got['weight']}")
    check("docx: the justified paragraph is justified", got["align"] == "justify", f"{got['align']}")
    img = got["img"]
    check("docx: the header image draws from a blob address", bool(img) and img["src"] == "blob:" and img["w"] > 0, f"{img}")
    errors = [w for w in warnings if "Content Security Policy" in w or "docx failed" in w]
    check("docx: no policy error and no draw error", not errors, f"{errors[:1]}")
    page.screenshot(path=str(SHOTS / "wb_viewer_docx.png"))


def check_archive(page, slug):
    open_path(page, slug, "bundle.zip")
    sel = pane(slug, "bundle.zip")
    page.wait_for_selector(f"{sel} .archive-head", timeout=15000)
    got = page.evaluate(
        f"""() => {{
        const body = document.querySelector('{sel} .archive-body');
        const rows = [...body.querySelectorAll('.archive-row:not(.archive-head)')];
        const file = rows.find(r => r.textContent.includes('file00.txt'));
        body.scrollTop = body.scrollHeight;
        const head = body.querySelector('.archive-head').getBoundingClientRect().top;
        return {{
          scrolls: body.scrollHeight > body.clientHeight && body.scrollTop > 0,
          headTop: Math.round(head - body.getBoundingClientRect().top),
          name: file ? file.querySelector('.archive-path').textContent : null,
          size: file ? file.querySelector('.archive-size').textContent : null,
          indent: file ? file.querySelector('.archive-path span').getBoundingClientRect().left
            - body.querySelector('.archive-head .archive-path').getBoundingClientRect().left : null,
        }};
      }}"""
    )
    check("zip: the list scrolls and the header stays at the top", got["scrolls"] and got["headTop"] == 0, f"{got}")
    check("zip: a row shows its name and its size apart", got["name"] == "file00.txt" and got["size"] == "1,000 bytes", f"{got}")
    check("zip: a file is indented under its folders", (got["indent"] or 0) > 20, f"{got['indent']}")
    page.screenshot(path=str(SHOTS / "wb_viewer_zip.png"))


def check_csv(page, slug):
    open_path(page, slug, "money.csv")
    sel = pane(slug, "money.csv")
    page.wait_for_selector(f"{sel} .sheet-switch", timeout=15000)
    # The switch reads the editor's text, so it waits for the editor to hold it.
    page.wait_for_function(
        f"() => (document.querySelector('{sel} .view-lines')?.textContent ?? '').includes('name;value')",
        timeout=15000,
    )
    page.click(f"{sel} .sheet-switch")
    page.wait_for_selector(f"{sel} .tabulator-cell", timeout=30000)
    cells = lambda: page.eval_on_selector_all(
        f"{sel} .tabulator-row .tabulator-cell:not(.sheet-rownum)", "els => els.map(e => e.textContent)"
    )
    first = cells()
    check("csv: a semicolon file splits on semicolons", first[:4] == ["name", "value", "fig", "3,5"], f"{first[:4]}")
    ground = page.eval_on_selector(f"{sel} .tabulator-row", "el => getComputedStyle(el).backgroundColor")
    check("csv: the grid has the workbench colors, not the light sheet", ground != "rgb(255, 255, 255)", ground)
    page.select_option(f"{sel} .sheet-separator", ",")
    page.wait_for_function(
        f"() => [...document.querySelectorAll('{sel} .tabulator-cell')].some(c => c.textContent === 'fig;3')"
    )
    check("csv: the separator selector changes the split", True)
    page.screenshot(path=str(SHOTS / "wb_viewer_csv.png"))


def check_odt(page, slug):
    open_path(page, slug, "notes.odt")
    sel = pane(slug, "notes.odt")
    page.wait_for_selector(f"{sel} .odt-page p", timeout=30000)
    page.wait_for_function(f"() => document.querySelector('{sel} .odt-page img')?.naturalWidth > 0", timeout=15000)
    got = page.evaluate(
        f"""() => {{
        const root = document.querySelector('{sel} .odt-page');
        const spans = [...root.querySelectorAll('span')];
        const bold = spans.find(s => s.textContent === 'Bold words');
        const marked = spans.find(s => s.textContent === 'Marked words'.toLowerCase());
        const lists = [...root.querySelectorAll('ol')];
        return {{
          bold: bold ? getComputedStyle(bold).fontWeight : null,
          marked: marked ? getComputedStyle(marked).backgroundColor : null,
          starts: lists.map(l => l.start),
          font: getComputedStyle(root).fontFamily,
          ground: getComputedStyle(root).backgroundColor,
        }};
      }}"""
    )
    check("odt: bold text is bold", got["bold"] == "700", f"{got}")
    check("odt: a highlight keeps its color", got["marked"] == "rgb(255, 255, 0)", f"{got['marked']}")
    check("odt: a continued list goes on counting", got["starts"] == [1, 3], f"{got['starts']}")
    check("odt: the page takes the file's font on white", "Liberation Serif" in got["font"] and got["ground"] == "rgb(255, 255, 255)", f"{got['font']} {got['ground']}")
    check("odt: the image draws", True)
    page.screenshot(path=str(SHOTS / "wb_viewer_odt.png"))


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
            check_docx(page, slug, warnings)
            check_archive(page, slug)
            check_csv(page, slug)
            check_odt(page, slug)
            browser.close()
    finally:
        stop(proc)
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if len(results) == 26 and all(results) else 1)


if __name__ == "__main__":
    main()
