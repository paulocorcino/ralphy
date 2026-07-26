# A vendored Excalidraw whiteboard — considered and rejected

Status: **rejected** (2026-07-26). Decided, then dropped the same day, before any
implementation existed. No code, no vendored bytes, no protocol change ever
landed; the three companion amendments this decision needed (ADR-0035, ADR-0036,
ADR-0037) were written and then removed, and those ADRs stand exactly as they did
before. PRD #321 and issues #322–#326 are closed.

This file is kept, rather than deleted, for one reason: the research behind the
rejection was expensive, and the next person to ask "why doesn't the workbench
have a whiteboard?" deserves the measurements rather than a second pass at
deriving them.

## The question

The workbench can show code (Monaco) and rendered prose with mermaid fences. It
cannot let a human think spatially with a mouse. The proposal was a whiteboard
in `docs/whiteboards/`, drawn with a vendored Excalidraw, saved as a scene plus a
rendered picture an agent could read.

## Why it was rejected

**The premise was inferred, not stated.** The decision rested on "the operator
wants to *draw*", which came from watching which tools were reached for
(Excalidraw, Penpot, tldraw — all free-form canvases), not from a stated need.
When the concrete use case was finally named, it was **"explain business logic to
the agent"** — and that is precisely the case where drawing loses.

**For that use case, mermaid wins outright, and is already here.** Business logic
lives on conditions and edge cases. A picture carries topology; it cannot carry
*"if amount > 1000 **and** the customer is unverified, then"* without putting text
inside an image — which is worse than text in a file on every axis that matters
here: not greppable, not diffable, not quotable back as an acceptance criterion,
and read by recognition rather than by character. Mermaid is precise, diffs by
line, and — unlike any whiteboard — an agent **reads and writes it**, so the
channel is bidirectional in a way a drawing never could be.

**The gap was real but somewhere else.** The markdown pane is a **toggle, not a
split**: `.md-split.editing` sets `display: none` on the preview, so authoring a
diagram means typing blind and alternating to look. That friction is a plausible
explanation for why exactly one file in this repo carries a mermaid fence. A CSS
split and a debounced re-render address it, for every markdown file in the repo,
with no vendoring and no Rust.

**The cost comparison was not close.** ~3.5 MB vendored plus React, a one-off
`esbuild`, three unproven gates, a new byte-write seam in the daemon and five
issues — against a split pane and a debounce.

**And it would have been built on hope.** ADR-0036's amendment refused to widen
eight agent charters to advertise a directory nobody had used yet, on the grounds
that it was a change made on hope. Vendoring an entire React application in the
hope that a sketching need would materialise is the expensive version of the same
mistake.

## What was measured, and is worth not re-deriving

**`@excalidraw/excalidraw` 0.18.1 (MIT), `dist/prod`: 18 MB / 300 files.**

| Part | Size | Files |
| --- | --- | --- |
| `fonts/Xiaolai/` (one CJK family) | 12.00 MB | 209 |
| `locales/` | 1.57 MB | 55 |
| core JS (`chunk-EIO257PC.js` alone is 1.74 MB) | 2.66 MB | 9 |
| `index.css` | 0.14 MB | 1 |
| Latin faces | ~0.50 MB | 25 |

Pruned to ~3.3 MB / ~90 files — against Monaco's 5.6 MB and mermaid's 3.5 MB
already embedded. **Size was never the objection.**

**The React distribution is the real tax, and it is not obvious.** Excalidraw's
`dist/prod` externalises React as bare ESM specifiers — `from"react"` ×92,
`from"react/jsx-runtime"` ×158, `from"react-dom"` ×6 — so an importmap must
resolve all three **as ES modules**. It cannot: `react@19.2.0` ships `cjs/` and
nothing else, and `react@18.3.1`'s UMD is not ESM and has no `react/jsx-runtime`
entry at all, so a hand-written bridge would mean reimplementing a React
internal. Excalidraw's own "no bundler" example works only because it fetches
`esm.sh`, which transforms CJS to ESM **in flight, from a CDN** — unacceptable at
runtime here, and not re-derivable as a vendored artifact. The workable path was
a single `esbuild` at vendoring time, committed and documented.

**Three claims were never measured**, and stayed the decision's open risk: that
Excalidraw's ESM coexists with Monaco's AMD loader on one page; that the font
prune survives Excalidraw's export-time subsetting; and that the pane makes zero
network requests when self-hosted.

**A local constraint worth remembering regardless of tool:** `tree::read` caps one
read at `MAX_READ_BYTES` = 2 MB and refuses non-UTF-8. Any scene format reaches
that through freehand strokes, which serialize as point arrays. And the Write
effect class is **text-only** in implementation (`fswrite::write` takes `&str`)
despite its amendment saying "bytes" — so any binary artifact written from the
workbench needs that gap closed first.

## The alternatives, and why each lost

**JSON Canvas** ([spec](https://jsoncanvas.org/spec/1.0/), MIT, Obsidian). Nearly
adopted before the realisation that it is **not a drawing format**: its four node
types (`text`, `file`, `link`, `group`) are all rectangles holding content — no
line, no ellipse, no freehand. It would have delivered a card board. Its **`file`
node** — a node that *is* a repo file, with a `#subpath` anchor — remains the best
idea encountered in this design and is worth reviving if a cards-and-arrows need
ever appears on its own.

**tldraw.** Excluded on licence, not merit. Its
[bespoke non-OSI licence](https://tldraw.dev/legal/tldraw-license) requires a
"made with tldraw" watermark for free use, and the SDK embeds measures that
validate a licence key and **detect the deployment environment**. A daemon
shipping TOTP, a host allowlist and no outbound telemetry cannot host a canvas
that phones home.

**Penpot.** A ClojureScript SPA plus a **JVM backend, PostgreSQL, S3-compatible
storage, Valkey and a Node/Puppeteer exporter** — a product you deploy, beside a
daemon that is one Rust binary. It is also a Figma alternative: interface design,
not idea sketching.

**Our own engine over Konva/Fabric.** ~50 KB and full control, against writing and
maintaining selection, resize handles, snapping, text editing, z-order, groups and
undo — an order of magnitude more work than everything else combined, for an
editor that would never be as good.

## What would revive this

Observed, not anticipated, need to **arrange in space** — the thing mermaid
structurally cannot do, because it chooses positions for you. Splitting a canvas
into "what exists" and "what's missing" and having the split *be* the idea;
circling three things to say they share a bug; an arrow from a scribble to a box.

If that shows up in real use after mermaid authoring is comfortable, the research
above is done and this decision can be reopened rather than rebuilt. Until then,
it is a solution looking for its problem.
