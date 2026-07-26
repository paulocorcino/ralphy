# The whiteboard is a vendored Excalidraw; the repo keeps the source and a picture

Status: **proposed** (2026-07-26) — decided in design, not yet implemented.

_Three ADRs carry a companion amendment dated 2026-07-26 that makes this
implementable: [ADR-0036](./0036-workbench-daemon-integration-protocol.md) gains
the `encoding` field on `file.write` (§5 here);
[ADR-0035](./0035-daemon-ui-visual-language.md) records the whiteboard pane's
palette exemption (§8 here); [ADR-0037](./0037-workbench-canvas-tabbed-workspace.md)
records the fourth pane kind (§4 here). Each is additive — no frozen section is
reopened, and no existing behaviour changes. This ADR changes nothing in
`ralphy-core` and adds no adapter surface._

The workbench can show code (Monaco) and rendered prose with mermaid fences. It
has no way for a human to **think spatially with a mouse** — to put this near
that, to circle three things, to sketch the shape of a design before it has a
name. Every diagram in the repo today must be typed.

The stated purpose is narrower and more useful than "a drawing tool": a whiteboard
is **a place where a human and an agent hand each other context**. That framing
decided most of what follows, and it cuts both ways — an agent must be able to
*receive* what was drawn, which is a harder requirement than it sounds.

## Decision

### 1. The whiteboard is Excalidraw itself, vendored and pinned

Not a format we invent, and not a canvas engine we write. `@excalidraw/excalidraw`
**0.18.1**, MIT, copied into `assets/ui/vendor/excalidraw/` the way Monaco was
([WORKBENCH-BUILD-GUIDE.md](../WORKBENCH-BUILD-GUIDE.md) §Vendored Monaco):
pinned, pruned by a prefix rule, and locked by a Rust embed-pin test.

The measured cost, from the npm tarball:

| Part | Size | Files | Kept? |
| --- | --- | --- | --- |
| `fonts/Xiaolai/` | **12.00 MB** | 209 | **no** — one CJK family, 96% of all font weight |
| `locales/` | 1.57 MB | 55 | `en` only (~30 KB) — English is canonical (CLAUDE.md) |
| core JS | 2.66 MB | 9 | yes (`chunk-EIO257PC.js` alone is 1.74 MB) |
| `index.css` | 0.14 MB | 1 | yes |
| Latin fonts | ~0.50 MB | 25 | yes (Excalifont, Virgil, Nunito, ComicShanns, Lilita, Cascadia…) |

**Pruned set ≈ 3.3 MB / ~90 files** — less than the already-embedded mermaid
(3.5 MB) and well under Monaco (5.6 MB). Size is not an objection here; the
precedent covers it.

One honest difference from the Monaco prune: that one cut **inert** files
(language workers nothing requested). Dropping Xiaolai is a **functional** cut —
CJK text in a scene falls back to another face. Accepted deliberately; 12 MB for
one Chinese handwriting family is not a trade this tool should make.

### 2. Vendoring costs one `esbuild`, run once, and never at build or CI time

Excalidraw's `dist/prod` externalizes React as **bare ESM specifiers** —
`from"react"` ×92, `from"react/jsx-runtime"` ×158, `from"react-dom"` ×6 — so a
browser importmap must resolve all three **as ES modules**. It cannot:

```
react@19.2.0  → package/cjs           (that is all)
react@18.3.1  → package/cjs  package/umd
```

React 19 publishes neither UMD nor ESM. React 18's UMD is not ESM, and
`react/jsx-runtime` — the module behind 158 of those imports — has no UMD entry
at all, so a hand-written bridge would mean reimplementing a React internal.
Excalidraw's own "no bundler" example works only because it fetches from
`esm.sh`, which transforms CJS to ESM **in flight, from a CDN**.

So: **run `esbuild` once, at vendoring time, commit the ESM output, and document
the exact command** in [WORKBENCH-BUILD-GUIDE.md](../WORKBENCH-BUILD-GUIDE.md)
beside the Monaco prune. CI stays node-free, the binary stays single, no CDN is
contacted at runtime, and the provenance stays re-derivable — which vendoring a
CDN's transform output would not be. React + ReactDOM minified add ~200 KB.

**Load-order note:** Monaco's AMD loader installs a global `define` with
`define.amd`, and the build guide records that every UMD vendor must load before
it. Excalidraw arrives as ESM through an importmap, a different mechanism that
should not collide — **but this must be proven on the page, not assumed.**

### 3. Pinned and left alone

There is no update treadmill. The version is pinned, the file set is locked by
test, and our own code touches the **smallest possible slice of their API** —
mount, load a scene, read a scene, export. The less of their surface we use, the
cheaper a future bump is, and a bump happens only when there is a reason.

### 4. The artifact is a pair: `.excalidraw` for the source, `.png` for the picture

A whiteboard in `docs/whiteboards/` is two files written by the **same save**:

- **`<name>.excalidraw`** — the scene, JSON, the source of truth. Text, so
  `file.read` serves it and git diffs it element by element.
- **`<name>.png`** — the rendered picture, derived and disposable. Committed.

**Why PNG and not SVG.** An agent's file-reading tool reads **text**. Handed an
SVG it gets XML: it can recover the `<text>` labels, but position, grouping and
what an arrow points at become inference over `path` coordinates — which is
precisely the ambiguity that ruled out a geometry-first format in the first place.
A PNG is *seen*: a multimodal model ingests it as the picture that was drawn.
GitHub renders both. The two usual arguments for SVG do not survive contact here
— a regenerated companion is a whole-file rewrite either way, so its diff was
never going to be readable, and nothing ever reads the companion back, so the
daemon's binary-read refusal is irrelevant.

**Why not one file with the scene embedded.** Excalidraw's `exportEmbedScene`
would make a single `.excalidraw.svg` that is both source and picture. It is
rejected on a hard local limit: [`tree.rs`](../../crates/ralphy-daemon/src/tree.rs)
caps one `file.read` at `MAX_READ_BYTES` = 2 MB, and an Excalidraw SVG carrying
text also embeds its fonts as base64. The workbench could write a file it can no
longer open. Keeping the source lean keeps it readable.

**The picture is derived, and that is a feature:** if this pane is ever replaced,
the `.png` regenerates and nothing is lost.

**The 2 MB cap binds the source too, and that is the known bound of this
decision.** The cap is on `file.read`, so it applies to every `.excalidraw` the
workbench opens — not only to the embedded-scene variant rejected above. A scene
reaches it through **freedraw**: a hand-drawn stroke serializes as an array of
points, and a long sketching session accumulates them fast, in a way that boxes
and arrows never would. Thousands of elements are needed to get there, so no
ordinary board is at risk — but a board that gets there becomes **unopenable by
the workbench that wrote it**, which is the worst failure shape available.

Accepted as a bound rather than engineered around, on purpose: raising
`MAX_READ_BYTES` would loosen a limit that guards *every* read for the sake of
one pane, and a per-verb cap is a second constant to keep in step. What the
implementation owes instead is that the failure be **legible**: hitting it must
surface "too large" against the whiteboard, not a blank canvas. If a real board
ever hits it, that is the evidence that reopens this paragraph.

### 5. `file.write` gains an `encoding`, because a PNG is not a string

Today the Write class is text-only: `fswrite::write(root, rel, content: &str)`,
and the verb reads `payload.content` with `as_str()`. A PNG cannot pass through
it. `file.write` therefore takes an optional **`encoding`** field — absent or
`"utf-8"` means today's behaviour; `"base64"` means decode the payload to bytes
before the confined write. Recorded as an amendment on the ADR that owns the
protocol ([ADR-0036](./0036-workbench-daemon-integration-protocol.md), amendment
of 2026-07-26), which carries the boundaries in full.

An `encoding` field rather than a new verb: the effect class, the confinement,
the protected-dir denylist and the refusal vocabulary are **identical** — only
the decoding of one payload field differs. A second verb would duplicate the
whole security path to express nothing new (`anti-over-abstraction`).

Two properties are load-bearing and belong to the implementation: decoding
resolves **no** path, so a base64 write can reach nothing a UTF-8 write could
not; and base64 that fails to decode is a **refusal with the file untouched**,
never a truncation discovered halfway.

### 6. Each direction keeps the medium it is good at

- **human → agent**: draw. The agent is handed the `.png` and sees it.
- **agent → human**: unchanged — markdown with a mermaid fence, which the
  workbench already renders ([`wb-viewer.js`](../../crates/ralphy-daemon/assets/ui/wb-viewer.js)
  turns every ` ```mermaid ` fence into a diagram, sanitized `strict`).

An agent cannot author an image, and forcing one artifact to carry both
directions would have made it bad at each. This is the whole reason the
whiteboard is allowed to be a *drawing* tool rather than a boxes-and-arrows one:
it does not have to be machine-writable, because the machine already has a
channel it writes well.

### 7. The agent reaches a whiteboard by being pointed at it

No charter changes in the first phase. The plan and execute prompts list what an
agent reads (`CLAUDE.md`, `CONTEXT.md`, `docs/adr/`) and that list is closed and
**per-adapter on purpose**; widening eight prompts to advertise a directory
nobody has used yet is a change made on hope. A human references
`docs/whiteboards/<name>.png` in an issue and the agent reads it because it was
named. If the habit takes, adding the directory to the charters becomes a
decision with evidence behind it.

### 8. The pane is local-first, and does not wear our palette

Excalidraw ships its own UI, including affordances that point at a cloud —
live collaboration, share links, Excalidraw+ promotion. Those are **disabled at
mount**, not styled away: a daemon whose posture is offline and operator-owned
must not render a button that would send a drawing somewhere.

The pane also keeps Excalidraw's own dark theme rather than the ADR-0035 warm
palette. This is a **conscious exemption**, not an oversight — retinting a
vendored React app's internals is exactly the deep coupling §3 exists to avoid,
and it is the kind of cost that makes a version bump expensive.

## Rejected alternatives

**JSON Canvas** ([spec](https://jsoncanvas.org/spec/1.0/), MIT, Obsidian). Very
nearly adopted, and worth recording why not: it is **not a drawing format**. Its
four node types (`text`, `file`, `link`, `group`) are all rectangles holding
content — no line, no ellipse, no freehand, no arbitrary shape. It would have
delivered a card board, not a whiteboard. Its `file` node — a node that *is* a
repo file, with a `#subpath` anchor — remains the best idea encountered in this
design, and is worth revisiting if the cards-and-arrows need ever appears
separately.

**tldraw.** Excluded on licence, not merit. The SDK is under a
[bespoke non-OSI licence](https://tldraw.dev/legal/tldraw-license): free use
requires a "made with tldraw" watermark on the canvas, and the SDK embeds
technical measures that validate a licence key and **detect the deployment
environment**. A local-first daemon that ships TOTP, a host allowlist and no
outbound telemetry cannot embed a canvas that phones home or brands the
operator's own screen.

**Penpot.** Not embeddable and the wrong category. Its architecture is a
ClojureScript SPA plus a **JVM backend, PostgreSQL, S3-compatible storage,
Valkey, and a Node/Puppeteer exporter** — a product you deploy, next to a daemon
that is one Rust binary. It is also a Figma alternative: interface design, not
idea sketching.

**Our own engine over Konva/Fabric.** ~50 KB instead of 3.5 MB, and a format we
control. Rejected because a *good* drawing editor — selection, resize handles,
snapping, text editing, z-order, groups, undo — is an order of magnitude more
work than the rest of this decision combined, and the result would never be as
good as the one that is free.

**Mermaid as the whiteboard format.** No positions exist in mermaid, so an
arrangement cannot be recorded; every open would re-lay-out from scratch, and a
GUI round-trip loses comments and ordering. Mermaid is not displaced by this
decision — §6 keeps it as the agent→human channel, and a mermaid fence can live
inside a whiteboard's text.

**Rendering as the only channel.** Keeping just the picture and dropping the
source reads well until the agent must *edit* a board: it cannot write an image.
§4 keeps both.

## Consequences

- `assets/ui/` grows ~3.5 MB and a vendored React. The build guide gains an
  Excalidraw section beside Monaco's, carrying the prune rule **and the exact
  `esbuild` invocation**, and a Rust embed-pin test locks the file set the way
  `monaco_replaced_codemirror_in_the_embedded_ui` does.
- `classify()` learns `.excalidraw`; `wb-viewer.js` gains a fourth tab kind
  beside code, markdown and diff.
- The daemon gains **no verb**. It gains one optional field on `file.write`, and
  `fswrite::write` gains a bytes path beside the string one — under the same
  confinement, the same denylist, the same refusal vocabulary.
- A save writes **two** files. They can only drift if one write fails; the save
  path must therefore report a partial write rather than swallow it.
- `docs/whiteboards/` starts carrying committed PNGs. Whiteboards are for
  thinking, not for build output; if the directory ever grows heavy, that is a
  signal to prune boards, not to un-commit the pictures — an uncommitted picture
  is invisible to the agent and to GitHub, which is the entire point.

## What must be proven before this is called done

Three things in this decision are **reasoned, not measured**, and each can
invalidate a piece of it. They are named here so the implementation treats them
as gates rather than discovering them late:

1. **ESM and the AMD loader coexist on one page.** Monaco's loader installs a
   global `define` with `define.amd`, and the build guide records that every UMD
   vendor must load before it. Excalidraw arrives as ESM through an importmap —
   a different mechanism that *should* be untouched by that, but the page is the
   only authority. Prove it with both a Monaco tab and a whiteboard tab open at
   once, not with one alone.
2. **The prune is survivable.** `subset-worker.chunk.js` does font subsetting on
   export; dropping 209 Xiaolai files and 54 locales must not make it throw, and
   an export must still produce a correct PNG. Verify against a board that has
   text in it, which is the case that reaches for a font at all.
3. **The offline claim holds.** With `EXCALIDRAW_ASSET_PATH` pointed at the
   vendored directory, the pane must make **zero network requests** — checked on
   the network panel with the machine's own daemon, not assumed from
   configuration. A single leaked font fetch would break the property this whole
   tool is built on.
