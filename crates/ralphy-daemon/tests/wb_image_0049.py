"""ADR-0049 browser acceptance: the workbench displays an image.

One Playwright pass over a REAL daemon proving a `.png` in the tree opens an
image tab instead of flashing `binary`, that the bytes really decode (intrinsic
size read off the live `<img>`), that a file whose bytes belie its `.png` name is
refused, that a non-image binary still refuses, that the pane offers no write
control, that the Actual-size toggle flips, and that a markdown preview resolves
its repo-relative image source while leaving a remote one alone.

Scenario 1  `logo.png` opens `file:<slug>:logo.png` at tabs[1] with `.image-viewer`
            mounted, `img.img-canvas` carrying a `data:image/png;base64,` src
Scenario 2  that image DECODES: naturalWidth/naturalHeight === the fixture's 64×48,
            and the toolbar's `.img-meta` says so
Scenario 3  `badge.svg` opens too, as `data:image/svg+xml;base64,`
Scenario 4  `evil.png` (HTML bytes under a `.png` name) flashes `not an image`
            and leaves no tab — the magic-byte check, end to end
Scenario 5  `notes.pdf` still flashes `binary`: the refusal narrowed, it did not go
Scenario 6  the image pane exposes no Save/Edit/commit control (read-only, §6)
Scenario 7  Actual size toggles `.actual-size` on the pane and relabels to `Fit`
Scenario 8  `docs/guide.md`'s `![](img/inner.png)` becomes a decoded `data:` URL
            resolved against the DOCUMENT's dir, while the `https://` source is
            left verbatim
Scenario 9  the REST of the allowlist — jpg, jpeg, gif, webp, bmp, ico — each
            opens under its own media type and decodes at its own distinct size

No image fixture is checked in: the PNGs are hand-encoded (zlib + CRC32) and the
rest are written by Pillow at fixture time, so "the browser decoded it" is a real
oracle rather than a re-read of our own bytes.

Requires `playwright` and `pillow` (`pip install playwright pillow`, then
`playwright install chromium`).

Boots a Localhost daemon on 7413 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/0049-image-viewer-2026-07-26.png.
Run: python crates/ralphy-daemon/tests/wb_image_0049.py   (exit 0 = all pass)
"""

import io
import os
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import zlib
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7413
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_image_0049.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

LOGO_W, LOGO_H = 64, 48
INNER_W, INNER_H = 32, 24

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def png_bytes(width, height, rgb=(232, 217, 168)):
    """A real, decodable RGB PNG — signature, IHDR, IDAT, IEND with live CRCs.
    Hand-encoded so the browser's `naturalWidth` is an independent oracle."""

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


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
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb0049_empty_")
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


# The rest of the allowlist, each at a DISTINCT intrinsic size so a pane that
# painted the wrong file's bytes reds instead of passing on a coincidence.
# `.jpg` and `.jpeg` are both here because they are two extensions the daemon
# folds onto one type, and a fold is exactly where a mapping goes wrong.
SWEEP = [
    ("shot.jpg", "JPEG", "image/jpeg", 48, 32),
    ("photo.jpeg", "JPEG", "image/jpeg", 56, 24),
    ("anim.gif", "GIF", "image/gif", 40, 28),
    ("card.webp", "WEBP", "image/webp", 36, 20),
    ("old.bmp", "BMP", "image/bmp", 44, 16),
    ("fav.ico", "ICO", "image/x-icon", 32, 32),
]


def sweep_bytes(fmt, width, height):
    """A real file in `fmt`, encoded by Pillow. Unlike the hand-rolled PNG these
    formats have no honest short encoder (JPEG's entropy coding, WebP's VP8
    bitstream), and a fixture that only carried the right MAGIC would prove the
    daemon's check while proving nothing about the browser's decode."""
    from PIL import Image

    img = Image.new("RGB", (width, height), (232, 217, 168))
    buf = io.BytesIO()
    # ICO stores a size list; pin the one size so `naturalWidth` is determinate.
    img.save(buf, format=fmt, **({"sizes": [(width, height)]} if fmt == "ICO" else {}))
    return buf.getvalue()


def make_fixture_repo():
    """A committed repo covering every arm of ADR-0049: one real file per
    allowlisted media type, HTML wearing a `.png` name, a non-image binary, and a
    markdown document whose image source is relative to its OWN directory (not
    the repo root)."""
    d = tempfile.mkdtemp(prefix="wb0049_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text("# fixture\n", encoding="utf-8")
    (p / "logo.png").write_bytes(png_bytes(LOGO_W, LOGO_H))
    for name, fmt, _media, w, h in SWEEP:
        (p / name).write_bytes(sweep_bytes(fmt, w, h))
    (p / "badge.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">'
        '<circle cx="20" cy="20" r="18" fill="#e8d9a8"/></svg>',
        encoding="utf-8",
    )
    # The magic-byte case: a `.png` whose bytes are a script-bearing document.
    (p / "evil.png").write_bytes(b"<html><script>alert(1)</script></html>")
    (p / "notes.pdf").write_bytes(b"%PDF-1.7\n\x00\x01binary trailer")
    (p / "docs").mkdir()
    (p / "docs" / "img").mkdir()
    (p / "docs" / "img" / "inner.png").write_bytes(png_bytes(INNER_W, INNER_H, (111, 159, 192)))
    (p / "docs" / "guide.md").write_text(
        "# Guide\n\n"
        "![shot](img/inner.png)\n\n"
        "![remote](https://example.invalid/nope.png)\n",
        encoding="utf-8",
    )
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb0049@example.com"],
        ["git", "config", "user.name", "wb0049"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's viewer.
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def tab_ids(page):
    return page.evaluate(f"() => {SH}.tabs.map(t => t.id)")


def open_from_tree(page, title):
    """Open a file the way a double-click does — through `openFile(node)`, so
    `classify` and its refusal are exercised, not bypassed by `openTab`."""
    page.wait_for_function(
        f"(t) => !!{SH}._tree && !!{SH}._tree.findFirst(n => n.title === t)",
        arg=title,
        timeout=20000,
    )
    page.evaluate(
        f"(t) => {SH}.openFile({SH}._tree.findFirst(n => n.title === t))", arg=title
    )


def wait_image_mounted(page, tab_id, timeout=20000):
    """Wait for the pane AND a settled decode: `naturalWidth` is 0 between the
    `src` assignment and the decode, so a wait on the element alone reads zero."""
    page.wait_for_function(
        "(id) => { const el = document.querySelector(`.image-viewer[data-tab-id=\"${id}\"]`);"
        " const img = el && el.querySelector('img.img-canvas');"
        " return !!img && img.complete && img.naturalWidth > 0; }",
        arg=tab_id,
        timeout=timeout,
    )


def expect_refusal(page, title, word):
    """Open a file expected to refuse, and wait for BOTH the flash and the close
    of the optimistically-pushed tab (reading the tab list mid-flight races)."""
    before = tab_ids(page)
    page.evaluate(f"() => {{ {SH}.runsActionMsg = ''; }}")
    open_from_tree(page, title)
    page.wait_for_function(
        f"(w) => ({SH}.runsActionMsg || '').includes(w)", arg=word, timeout=15000
    )
    page.wait_for_function(
        f"(want) => {SH}.tabs.map(t => t.id).join('|') === want",
        arg="|".join(before),
        timeout=8000,
    )
    return before


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb0049_reg_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1500, "height": 950})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)
            page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)

            # --- scenario 1: a `.png` opens an image tab ----------------------
            png_id = f"file:{slug}:logo.png"
            open_from_tree(page, "logo.png")
            wait_image_mounted(page, png_id)
            ids = tab_ids(page)
            check(
                "the image tab opens at tabs[1], after the fixed Consoles tab",
                ids == ["consoles", png_id],
                f"got={ids}",
            )
            shape = page.evaluate(
                """(id) => {
                  const el = document.querySelector(`.image-viewer[data-tab-id="${id}"]`);
                  const img = el.querySelector('img.img-canvas');
                  return { prefix: img.getAttribute('src').slice(0, 22),
                           w: img.naturalWidth, h: img.naturalHeight,
                           meta: el.querySelector('.img-meta').textContent.trim(),
                           kind: null };
                }""",
                png_id,
            )
            check(
                "the pane's `<img>` is a data: URL of the daemon's verified type",
                shape["prefix"] == "data:image/png;base64,",
                f"got={shape['prefix']!r}",
            )

            # --- scenario 2: the bytes really decode --------------------------
            check(
                "the browser DECODED the served bytes at the fixture's size",
                (shape["w"], shape["h"]) == (LOGO_W, LOGO_H),
                f"got={shape['w']}×{shape['h']} want={LOGO_W}×{LOGO_H}",
            )
            check(
                "the toolbar reports that intrinsic size",
                shape["meta"] == f"{LOGO_W} × {LOGO_H}",
                f"got={shape['meta']!r}",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "0049-image-viewer-2026-07-26.png"))

            # --- scenario 7: Actual size toggles ------------------------------
            zoom = page.evaluate(
                """(id) => {
                  const el = document.querySelector(`.image-viewer[data-tab-id="${id}"]`);
                  const btn = el.querySelector('[data-act="zoom"]');
                  const before = el.classList.contains('actual-size');
                  btn.click();
                  const on = { cls: el.classList.contains('actual-size'), label: btn.textContent.trim() };
                  btn.click();
                  return { before, on, off: el.classList.contains('actual-size'),
                           offLabel: btn.textContent.trim() };
                }""",
                png_id,
            )
            check(
                "Actual size toggles `.actual-size` on and back off",
                zoom["before"] is False and zoom["on"]["cls"] is True and zoom["off"] is False,
                f"got={zoom}",
            )
            check(
                "…and the button relabels to Fit while it is on",
                zoom["on"]["label"] == "Fit" and zoom["offLabel"] == "Actual size",
                f"got={zoom['on']['label']!r}/{zoom['offLabel']!r}",
            )

            # --- scenario 6: the pane is read-only ----------------------------
            controls = page.evaluate(
                """(id) => {
                  const root = document.querySelector(`.image-viewer[data-tab-id="${id}"]`);
                  const re = /save|edit|commit|discard|stage|delete/i;
                  const hay = (e) => [
                    e.children.length ? "" : e.textContent || "",
                    e.getAttribute('title') || "",
                    e.getAttribute('aria-label') || "",
                    e.getAttribute('data-act') || "",
                  ].join(" ");
                  return {
                    vbtns: Array.from(root.querySelectorAll('.vbtn')).map(b => b.textContent.trim()),
                    writers: Array.from(root.querySelectorAll('*')).filter(e => re.test(hay(e))).length,
                  };
                }""",
                png_id,
            )
            check(
                "the image toolbar is exactly Actual size · Reload · Detach",
                controls["vbtns"] == ["Actual size", "Reload", "Detach"],
                f"got={controls['vbtns']}",
            )
            check(
                "no save / edit / commit / discard control exists on this surface",
                controls["writers"] == 0,
                f"matches={controls['writers']}",
            )

            # --- scenario 3: an SVG opens as an SVG ---------------------------
            svg_id = f"file:{slug}:badge.svg"
            open_from_tree(page, "badge.svg")
            wait_image_mounted(page, svg_id)
            svg_prefix = page.evaluate(
                "(id) => document.querySelector(`.image-viewer[data-tab-id=\"${id}\"] img.img-canvas`)"
                ".getAttribute('src').slice(0, 26)",
                svg_id,
            )
            check(
                "an SVG is served as image/svg+xml inside an <img>",
                svg_prefix == "data:image/svg+xml;base64,",
                f"got={svg_prefix!r}",
            )

            # --- scenario 9: the WHOLE allowlist opens and decodes -------------
            # PNG and SVG above are the two formats with an easy encoder, so
            # stopping there would leave four of the seven types proven only by
            # the daemon's own magic check — never by a browser that painted them.
            for name, _fmt, media, want_w, want_h in SWEEP:
                tab = f"file:{slug}:{name}"
                open_from_tree(page, name)
                wait_image_mounted(page, tab)
                got = page.evaluate(
                    """([id, media]) => {
                      const img = document.querySelector(
                        `.image-viewer[data-tab-id="${id}"] img.img-canvas`);
                      const src = img.getAttribute('src');
                      return { typed: src.startsWith(`data:${media};base64,`),
                               prefix: src.slice(0, 30),
                               w: img.naturalWidth, h: img.naturalHeight };
                    }""",
                    [tab, media],
                )
                check(
                    f"{name} opens as {media} and decodes at {want_w}×{want_h}",
                    got["typed"] and (got["w"], got["h"]) == (want_w, want_h),
                    f"got={got['prefix']!r} {got['w']}×{got['h']}",
                )

            # --- scenario 4: bytes that belie the extension refuse ------------
            before = expect_refusal(page, "evil.png", "not an image")
            check(
                "HTML wearing a `.png` name flashes `not an image` and opens no tab",
                tab_ids(page) == before,
                f"before={before} after={tab_ids(page)}",
            )

            # --- scenario 5: a non-image binary still refuses -----------------
            before = expect_refusal(page, "notes.pdf", "binary")
            check(
                "a `.pdf` still refuses as `binary` — the refusal narrowed, it did not go",
                tab_ids(page) == before,
                f"before={before} after={tab_ids(page)}",
            )

            # --- scenario 8: markdown resolves its relative image -------------
            md_id = f"file:{slug}:docs/guide.md"
            page.evaluate(
                f"""() => {SH}.openTab({{ project: '{slug}', path: 'docs/guide.md',
                        title: 'guide.md', ftype: 'markdown' }})"""
            )
            page.wait_for_function(
                "(id) => { const el = document.querySelector(`.md-viewer[data-tab-id=\"${id}\"]`);"
                " const imgs = el && el.querySelectorAll('.md-body img');"
                " return !!imgs && imgs.length === 2"
                "   && imgs[0].getAttribute('src').startsWith('data:')"
                "   && imgs[0].complete && imgs[0].naturalWidth > 0; }",
                arg=md_id,
                timeout=20000,
            )
            md = page.evaluate(
                """(id) => {
                  const el = document.querySelector(`.md-viewer[data-tab-id="${id}"]`);
                  const imgs = el.querySelectorAll('.md-body img');
                  return { local: imgs[0].getAttribute('src').slice(0, 22),
                           w: imgs[0].naturalWidth, h: imgs[0].naturalHeight,
                           remote: imgs[1].getAttribute('src') };
                }""",
                md_id,
            )
            check(
                "a repo-relative markdown image resolves against the DOCUMENT's dir",
                md["local"] == "data:image/png;base64," and (md["w"], md["h"]) == (INNER_W, INNER_H),
                f"got={md['local']!r} {md['w']}×{md['h']} want={INNER_W}×{INNER_H}",
            )
            check(
                "a remote markdown image source is left verbatim",
                md["remote"] == "https://example.invalid/nope.png",
                f"got={md['remote']!r}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
