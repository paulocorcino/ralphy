# The file viewer reads the file bytes

Status: proposed
Kind: structural
Protects: consistency of the workbench; constraint security

Amends ADR-0049 (§2: an image opened in a tab is read from the file bytes route, not as base64 in a reply).

## Context

The workbench shows text, diffs, Markdown, Mermaid, DBML, HTML and images.
It refuses every other binary file. Issue #980 asks for a **file viewer** for
images with zoom, PDF, documents (docx, odt), sheets (xlsx, csv, tsv), audio
and video, fonts and archives. Every viewer is read-only, runs in the browser
and the daemon only, and calls no outside program or web service.

Two paths carry the bytes of a file today, and neither one fits:

- `file.read` and `file.image` answer with the whole file inside a JSON
  reply: decoded text up to 2 MiB (`tree::MAX_READ_BYTES`), or an image as
  base64 up to 4 MiB (`tree::MAX_IMAGE_BYTES`, ADR-0049 §2).
- `GET /preview/<token>/<path>` (ADR-0089 D3) answers with raw bytes, but it
  reads the whole file, has no `Range`, and its token gives a folder to an HTML
  document that nobody trusts. Its list of kinds is what such a document may
  load.

Neither path can stream a video or let a PDF load one page at a time.

Facts measured for this decision (2026-11-20):

- `pdfjs-dist` 6.5.438 (`npm pack`): `pdf.min.mjs` 464 KB, `pdf.worker.min.mjs`
  1.3 MB, `web/pdf_viewer.mjs` 316 KB with 168 KB of CSS, `standard_fonts/`
  820 KB, `openjpeg.wasm` + `jbig2.wasm` + `qcms_bg.wasm` 420 KB,
  `quickjs-eval.wasm` (the PDF script engine) 469 KB, `cmaps/` 1.5 MB. The
  option `isEvalSupported` and the `eval` font path are gone from 6.x.
- `@firecrawl/anydoc-wasm` 0.2.5: `anydoc_wasm_bg.wasm` 6.7 MB. Its
  `toDocument` returns a typed model with blocks, tables (with merged cells)
  and embedded images as bytes (`anydoc_wasm.d.ts`). It reads docx, odt,
  xlsx, ods and csv.
- `tabulator-tables` 6.7.1: `tabulator.min.js` 455 KB, CSS 28 KB, MIT, no
  `eval` and no `new Function`, with an ESM build. It has a download module.
- A sheet of 20 columns, drawn as one plain table in Playwright, takes 1.2 to
  3.1 s for 5,000 rows and 13.6 to 44.4 s for 65,536 rows. Drawn with only the
  visible rows, it takes 28 to 57 ms for 65,536 or 1,048,576 rows, in
  Chromium, Firefox and WebKit.
- The crates `zip`, `tar` and `flate2` are already in the workspace
  (`Cargo.toml`), used by `ralphy-cli`.

## Decision

**D1. The editor reads text; the file viewer reads the file bytes.** The
editor (Monaco, editable Markdown, notes) reads with `file.read`: decoded text,
the whole file, and what a save needs. A file viewer never reads with
`file.read` or `file.image`. It reads the **file bytes**, or, for an archive,
the listing of D7. The images inside a rendered Markdown article stay on
`file.image` (ADR-0049 §5) until a later change moves them.

**D2. One route serves the file bytes.** `GET /api/file/bytes` takes the repo
and the path as `file.read` takes them, and confines the path as `file.read`
confines it. It reads the page cookie, as every route under `/api` does. It
answers a `Range` request with `206`, `Accept-Ranges: bytes` and
`Content-Range`, and reads only the asked range from the disk. Every answer
says `Content-Disposition: inline`, `nosniff`, `no-store` and the media type
that D3 verified.

**D3. The route serves a closed list of kinds, and the bytes must agree with
the extension.** The extension proposes the kind and the first bytes decide
(ADR-0049 §3). Every other file is not found.

| Group | Extensions | Check on the first bytes | Cap on the file |
| --- | --- | --- | --- |
| Image | png jpg jpeg gif webp bmp ico svg avif | as `tree::ImageType` today; `ftyp avif` | 32 MiB |
| PDF | pdf | `%PDF-` | none |
| Document | docx odt | ZIP; for odt, the `mimetype` entry | 32 MiB |
| Sheet | xlsx ods | ZIP; for ods, the `mimetype` entry | 32 MiB (2 MiB since the amendment of 2026-11-20) |
| Text table | csv tsv | text, by the binary check of `file.read` | 32 MiB |
| Media | mp4 m4a webm mp3 wav ogg oga ogv | `ftyp`, EBML, `ID3` or an MPEG frame, `RIFF`…`WAVE`, `OggS` | none |
| Font | ttf otf woff woff2 | `00 01 00 00` or `true`, `OTTO`, `wOFF`, `wOF2` | 16 MiB |

The daemon checks only the container of a ZIP kind. A ZIP that is not the
document it claims fails in the viewer, which says so.

**D4. One answer carries at most 8 MiB.** A request for `bytes=0-`, or with no
`Range`, gets the first 8 MiB at most, with `206` when the file is larger. The
browser asks for the rest (RFC 9110). A kind with no cap on the file has a cap
on each answer.

**D5. The route answers only a fetch and a media element.** It refuses with
`403` a request whose `Sec-Fetch-Dest` is present and is not `empty`,
`image`, `video` or `audio`. So the browser never opens the bytes as a page, in
its own PDF viewer or in its own player, which offer a save. A request with no
`Sec-Fetch-Dest` is served: this rule is a convenience of the interface, not a
security boundary, because the operator owns the files.

**D6. A project of a peer is served through the daemon the browser talks
to.** For a repo `<daemon_id>/<slug>`, the local daemon asks the peer with
`GET /api/peer/file/bytes`, with the peer key of the other `/api/peer/*`
routes and the same `Range`. It passes the body on as it arrives and never
holds a whole file. The peer confines the path and checks the kind. The
deadline of ADR-0082 D2 is a deadline for the first byte and a deadline with
no byte. A stream has no deadline on its whole length. No new tunnel program
is added (ADR-0052, ADR-0067).

**D7. The daemon lists an archive, with the verb `file.entries`.** It answers
zip, tar, tar.gz and tgz with each entry's path, size, compressed size (zip
only), folder flag and time, and a `truncated` flag. It reads the central
directory of a zip, and the whole stream of a tar.gz. It stops at 10,000
entries and at 1 GiB of inflated bytes, and says the list is partial. It never
writes an entry to disk and never opens an archive inside an archive. The
verb goes over `/api/command`, and through `/api/peer/command` for a peer.

**D8. Every library of a viewer is vendored, and loads on the first use.**
No viewer library is named by `index.html`. A viewer imports its library the
first time a file of its kind opens. Each library is in `vendor/manifest.json`
and its section in `docs/WORKBENCH-BUILD-GUIDE.md` (ADR-0072 D12).

**D9. A PDF is drawn by pdf.js, without its scripts.** The vendored parts are
the core, the worker, `pdf_viewer` (pages, the text layer, the search),
`standard_fonts` and the three image and color wasm files. `cmaps`,
`pdf.sandbox` and `quickjs-eval.wasm` are not vendored. pdf.js runs with
`enableScripting: false` and with no annotation editor. A form shows and is
never filled. A link to the web opens in a new tab with `noopener
noreferrer`. pdf.js reads the PDF in ranges from D2.

**D10. A document and a sheet are read by anydoc-wasm, in a worker, and drawn
from its model.** docx, odt, xlsx, ods, csv and tsv go to `toDocument` in a
worker. The viewer builds the DOM from the model: text with `textContent`,
an embedded image as a `blob:` address of its bytes. No HTML from the file
enters the page. A part the model marks as not readable shows a message in its
place.

**D11. A sheet is drawn by Tabulator.** The viewer imports the Tabulator core
from its ESM build and registers only the modules it uses. The download module
is never registered. Rows and columns are drawn virtually. A sheet shows the
stored value of each cell. No formula runs. A filter box searches every row of
the sheet. A sheet over 500,000 rows shows the first 500,000 and says how many
it has. A csv or tsv file opens in the editor as text, and a button changes it
to the sheet and back.

**D12. The file viewer offers no save.** No viewer has a button to download,
to print or to open the file outside the workbench. A media element has
`controlsList="nodownload noremoteplayback"` and `disablePictureInPicture`.
The browser's own context menu is turned off on a media element and on the
image of an image viewer. It stays on text (the PDF text layer, a document, a
sheet), where it is the Copy of the operator.

**D13. An image zooms in its pane, and every zoom uses one surface.** The
image viewer has a zoom bar: zoom out, the zoom percent, zoom in, Fit and
100%. The wheel zooms, two fingers pinch, one finger or the mouse drags. The
gestures are one function, used by the image viewer and by the Enlarge view
(`wb-zoom-overlay.ts`). The arithmetic stays in `wb-diagram-zoom.ts`. The PDF
viewer uses the same zoom bar on the scale of pdf.js. The zoom bar and its
keys get their entries in `docs/DESIGN-SYSTEM.md` and the keyboard command
list (ADR-0087) before any code.

**D14. Every size and count of this ADR is a constant** (ADR-0072 D11): the
cap of each kind in D3, the 8 MiB of D4, the 10,000 entries and 1 GiB of D7,
and the 500,000 rows of D11.

## Consequences

- The binary grows by about 12 MB: pdf.js about 3.5 MB, anydoc-wasm 6.7 MB,
  Tabulator about 0.5 MB, before compression. The page does not load any of
  it until a viewer needs it.
- An image of a peer project keeps working: today it comes through
  `/api/peer/command`, and D6 keeps a path for it.
- A file over 32 MiB of a whole-file kind is refused, as a file over 2 MiB is
  refused by the editor today.
- anydoc-wasm is three months old (first release 2026-08) and at version 0.x.
  Its API can change. The code that reads its model is the only code to change
  when it does.
- A PDF whose CJK text uses a font it does not embed, with an old Adobe
  encoding, can show wrong characters until `cmaps` is vendored.
- A `.ico` file shows the size the browser picks. The viewer does not choose
  another size inside the file.

## Considered options

- **Extend `file.read` with chunks of base64.** A media element cannot play a
  stream that a script assembles from JSON replies, and seeking would need code
  of our own.
- **Serve the viewer from `/preview/<token>/`.** That route gives a folder to an
  untrusted HTML document. Adding PDF, sheets and media to it adds them to what
  such a document may load. The two lists are two rules, so they are two
  routes.
- **Refuse a peer project, as ADR-0089 and ADR-0090 do.** Images of a peer
  project would stop opening, and an image would have two paths by where its
  project lives.
- **mammoth for docx and a second library for odt.** Two libraries for one
  viewer, HTML from the file that must pass DOMPurify, and the only maintained
  ODT reader found on npm (2026-11-20) has one maintainer and no
  built bundle. anydoc covers both and the sheets, and gives a model, not HTML.
- **anydoc as a Rust crate in the daemon.** A file built to hang the parser
  would hang the daemon that holds every console. It also adds `lopdf`,
  `rayon`, `env_logger`, `include_dir` and `ttf-parser` to the daemon, and its
  Markdown output keeps an image as its alt text only.
- **A virtual grid of our own.** It is small and was measured fast. Tabulator
  was chosen because it is maintained by others and already handles the edge
  cases.
- **The archive listed in the browser.** A tar.gz has no index: the browser
  would need the whole file, through a tunnel too. The daemon reads it as a
  stream with crates it already has.
- **The context menu turned off on the whole viewer.** It would remove Copy on
  text, which #980 asks for.

## Not measured

The spike before D10 and D11 measures these. A failure changes the decision
named.

- That anydoc-wasm gives one table per sheet of an xlsx and the stored values
  of formula cells (D10, D11). If it does not, the sheet gets another reader
  and documents stay on anydoc.
- That the embedded images of a real docx and a real odt arrive in `assets`
  (D10).
- The size of anydoc-wasm after gzip, and the time to read a 32 MiB file in the
  worker (D10).
- That the page policy lets the page compile WebAssembly in Chromium, Firefox
  and WebKit through `'unsafe-eval'` (D9, D10).
- That Tabulator draws 500,000 rows under the largest element height of each
  browser, and that its ESM build lets the viewer leave out the download module
  (D11).
- That Safari sends `Sec-Fetch-Dest` on a media request (D5).

## Compliance

- D1: not checked by code: manual, reviewed in the PR. A check that no viewer
  module calls `file.read` or `file.image` can be added with the first viewer.
- D2: not checked by code until the route exists. Its tests pin `206`,
  `Content-Range` and the confinement.
- D3: not checked by code until the route exists. A test pins that a file of a
  listed extension whose bytes disagree is not found.
- D4: not checked by code until the route exists.
- D5: not checked by code until the route exists. A browser check in
  `tests/browser/` pins that opening the address in a tab is refused.
- D6: not checked by code: needs a live peer; manual, reviewed in the PR.
- D7: not checked by code until the verb exists.
- D8: checked by the test `vendored_files_match_the_manifest` for the
  manifest. That no viewer library is named by `index.html`: not checked by
  code yet.
- D9: not checked by code: manual, reviewed in the PR (the options passed to
  pdf.js and the vendored file list).
- D10: not checked by code: manual, reviewed in the PR. Since the amendment
  of 2026-11-23, a unit test pins the options passed to docx-preview and the
  sanitizing of its output.
- D11: not checked by code: manual, reviewed in the PR (the registered
  modules).
- D12: not checked by code: manual, reviewed in the PR.
- D13: checked by `jscpd` on a pull request, which fails a second copy of the
  gestures.
- D14: not checked by code: manual, reviewed in the PR.

## Spike results (2026-11-20)

Issue #981. Playwright 1.60.0 for Python, its Chromium, Firefox and WebKit
builds on Windows 11. A local server sent the workbench page policy of
`routes/headers.rs` (without the script hashes). Libraries:
`@firecrawl/anydoc-wasm` 0.2.5 and `tabulator-tables` 6.7.1. Files: written
for the spike (a docx, an odt, an xlsx of three sheets, an ods of two sheets,
xlsx files of 5,000 to 50,000 rows, a CSV of 25 MiB and 400,000 rows), and
three docx and three xlsx files of the operator's computer, read locally;
only counts were kept.

- **The model.** An xlsx gives one heading (the sheet name) and one table per
  sheet; an xlsx with one sheet gives only the table. A formula cell holds
  its stored value (`A2*2` gives `42`). An ods gives the same shape. The
  embedded image of a docx and of an odt is in `assets`, with its media type.
  A real docx gave headings, lists and its 10 images.
- **WebAssembly under the page policy.** The module compiles and runs in a
  module worker in the three engines, through `'unsafe-eval'`, with no policy
  error in the console. First load: 57 to 109 ms.
- **Size.** `anydoc_wasm_bg.wasm` is 6.7 MB, 2.9 MB after gzip at the
  daemon's level (`Compression::default()`).
- **Time of `toDocument`, in a worker.**

  | File | Chromium | Firefox | WebKit |
  | --- | --- | --- | --- |
  | xlsx, 5,000 rows (0.2 MB) | 0.1 s | 0.6 s | 0.1 s |
  | xlsx, 20,000 rows (0.7 MB) | 0.4 s | 2.3 s | 0.4 s |
  | xlsx, 50,000 rows (1.8 MB) | 1.0 s | 5.7 s | 1.0 s |
  | CSV, 4 MiB (66,290 rows) | 1.0 s | 1.9 s | 22.8 s |
  | CSV, 8 MiB (130,614 rows) | 1.9 s | 4.1 s | 76.6 s |

  In Node 22, `toDocument` on the CSV of 25 MiB took 16.9 s and about 1 GB
  of heap. A 32 MiB sheet cannot be read this way.
- **Tabulator.** The ESM build with only `FilterModule`,
  `ResizeColumnsModule` and `ImportModule` registered has no `download`
  function. 500,000 rows of 5 columns: built in 0.2 to 0.4 s, the last row
  reached by scrolling, 47 row elements, a scroll height of 12,000,001 px,
  under the element height limit of each engine. A filter over the 500,000
  rows: 50 to 124 ms.
- **CSV by Tabulator itself.** `importFormat: "csv"` read the 8 MiB CSV
  (130,613 rows) in 0.27 s in Chromium, 0.47 s in Firefox and 0.31 s in
  WebKit.
- **`Sec-Fetch-Dest`.** Chromium and Firefox send `image`, `video` and
  `audio` for those elements and `empty` for a fetch. The WebKit build sends
  `image` and `empty`, and **no** `Sec-Fetch-Dest` on the requests of
  `<video>` and `<audio>`. D5 serves a request with no header, so media plays
  there. Safari on macOS and iOS was not measured: the WebKit build of
  Playwright on Windows is not Safari.

## Amendment 2026-11-20 — D3, D10, D11: a sheet by its kind, after the spike

The spike above measured that `toDocument` cannot read a large sheet in
time. The operator approved these changes on 2026-11-20.

**D3, changed.** The cap of an xlsx or an ods file is 2 MiB, not 32 MiB. An
xlsx of 1.8 MB (50,000 rows) took up to 5.7 s (Firefox). csv and tsv keep
32 MiB.

**D10, changed.** docx, odt, xlsx and ods go to anydoc-wasm. csv and tsv do
not.

**D11, changed.** Tabulator reads a csv or tsv file itself, with its
`ImportModule` (`importFormat: "csv"`), which is registered beside the
filter and column modules. An xlsx or ods file over 2 MiB shows that it is
too large to show, and no reader starts.

## Amendment 2026-11-20 — D8, D12: the media player is media-chrome

The operator chose `media-chrome` (MIT, version 4.19.5, 176 KB as its IIFE
build, no `eval` and no `new Function`) for audio and video, over the
browser's own controls. It draws its controls around the native `<video>`
or `<audio>`, which still reads the file bytes in ranges. `howler.js` was
looked at and left out: it plays audio from code and draws no player.

**D8, changed.** `media-chrome` is a vendored library of the media viewer.

**D12, changed.** A media file shows the controls of `media-chrome`, with no
download control among them, the same in every engine. Its keys are turned
off (`nohotkeys`), so no key of the player escapes the keyboard command list
(ADR-0087). The media element has no `controls` attribute; it keeps
`disablePictureInPicture`, and the browser's own context menu is off on the
player.

## Amendment 2026-11-23 — D8, D10: a docx is drawn by docx-preview

The model of anydoc-wasm keeps the words of a docx and four marks of a
character (bold, italic, strike, code). It has no header, footer, page,
font, size, color or alignment. A contract opened from a real project lost
its logo and its layout. The operator asked for a docx that looks like the
file in Word, as a PDF does, and approved these changes on 2026-11-23.

**Spike (2026-11-23).** `docx-preview` 0.4.1 (Apache-2.0, 75 KB minified,
with `jszip` 3.10.2, 98 KB) read a real contract of 130 KB in Chromium,
under a CSP with `script-src 'self'`. Reading took 35–51 ms and drawing
14–27 ms. The library has no `eval` and no `new Function`; jszip has one
`new Function`, in a `setImmediate` shim that runs only when it is given a
string, which the library never does. The library writes `innerHTML` only
to empty its containers and to write `&nbsp;`. An "altChunk" (HTML stored
in the docx) becomes an iframe with `srcdoc`: the option
`renderAltChunks: false` turns it off. DOMPurify removed 0.3 % of the
output and kept the layout. It removed the two images until a hook let a
`blob:` address through on an image. Compared with the PDF that Word
exported from the same file:

| | Word | docx-preview |
| --- | --- | --- |
| Fonts, bold, justified text, page width | yes | yes |
| Header image | in the header | drawn over the title |
| Footer in a text box | shown | empty |
| Pages | 17 | 2: only explicit breaks; the file has no saved page breaks |

The `experimental` mode drew fewer parts, not more. A copy that equals Word
needs a layout engine (LibreOffice in the daemon, about 300 MB on every
machine and peer). It was left out.

**D8, changed.** `docx-preview` and `jszip` are vendored libraries of the
document viewer, as one built file, as Crepe is. Neither can be copied byte
for byte: the ES module of docx-preview imports `jszip` by its bare name,
which a browser resolves only through an import map, and the page policy
refuses an inline script. Their UMD builds see the global AMD `define` of
the Monaco loader and register there instead of on `globalThis`. A recipe
in `crates/ralphy-daemon/vendor-build/docx-preview/` (esbuild, by hand,
never in CI) bundles the two into `vendor/docx-preview/docx-preview.js`,
whose first line names both versions.

**D10, changed.** A docx is drawn by `docx-preview`, with
`renderAltChunks: false`, `renderComments: false`, `renderChanges: false`
and `useBase64URL: false`. Its output passes DOMPurify, which keeps a
`blob:` address only as the source of an image, and then goes into a shadow
root of the pane, so the document's styles never reach the workbench. The
other rules of D10 stay: no script of the file runs, nothing loads from
outside the page, and a link to the web opens in a new tab with `noopener
noreferrer`. odt, xlsx and ods stay on anydoc-wasm.

**Considered.** mammoth was rejected again: it gives HTML with no layout.
A sandboxed iframe with no scripts was the fallback if DOMPurify had broken
the layout; it did not.
