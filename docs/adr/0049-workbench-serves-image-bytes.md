# The workbench serves image bytes: one Observe verb, base64, allowlisted types

Status: accepted (2026-07-26).

The workbench refuses to open an image. A `.png` in the tree carries an image
icon, and clicking it flashes `binary` and closes the tab it just opened — the
refusal is enforced twice, client-side (`BINARY_EXT` in `app.js`) and
daemon-side (`tree::read`'s NUL/UTF-8 sniff). The `docs/screenshots/` a run just
wrote are unreadable from the surface that wrote them, and a `![](…)` in a
repo `.md` renders as a broken image in the markdown preview, because no route
resolves a repo-relative image source to bytes.

This ADR decides how the daemon serves those bytes. It **amends
[ADR-0036](./0036-workbench-daemon-integration-protocol.md)** §2 (the Observe
class, so far text-only) and does not reopen §5 — confinement is unchanged and
remains the security boundary.

## Decision

### 1. Images are a viewer, not a refusal — and the refusal narrows, it does not disappear

The Observe class (ADR-0036 §2) reads "the working tree as OS bytes". Nothing in
that definition says *text*; the text-only shape is an artifact of `tree::read`
being the only reader built. This ADR adds a second reader under the same class:

> **`file.image`** (Observe) — read a confined repo path whose bytes are an
> **allowlisted image type**, and answer with the media type plus the bytes.

`file.read` is untouched. An image is never a text read, and a text file is
never an image read; the two readers refuse each other's inputs. Every
non-image binary — `.pdf`, `.zip`, `.exe`, a font, a media file — keeps refusing
exactly as today. The change is that "binary" stops being a synonym for
"unviewable".

### 2. Transport: base64 in the reply, rendered as a `data:` URL

The verb answers on the requesting `Command` id with
`{ status: "ok", mediaType, base64 }`, and the browser mounts it as
`<img src="data:<mediaType>;base64,<base64>">`. No new socket, no new route, no
new frame tag.

**Rejected: a raw HTTP byte route** (`GET /api/repos/{slug}/blob?path=…`
answering `Content-Type: image/png`). It is the obvious design and it is wrong
here for three independent reasons, any one of which decides it:

- ADR-0036 §1 exists to stop precisely this: capabilities are **rows in the verb
  registry, not routes**. A second door would re-implement the registry lookup,
  the confinement call, and the auth gate at a place that must now be kept in
  step with the first.
- It puts repo bytes at a **navigable same-origin URL**. An SVG opened as a
  top-level document on the daemon's own origin is a script-bearing document
  with access to the session cookie; that is stored XSS, and the mitigation
  (`Content-Disposition`, a sandboxed subdomain, CSP surgery) is a security
  surface we would then own forever. A `data:` URL inside `<img>` is inert by
  construction — it has no origin to be same-origin with.
- The reader would be `<img src>` issuing its own request, outside the socket
  the client already correlates replies on, so a refusal arrives as a broken
  image rather than a reason string the UI can flash.

**Rejected: a fourth binary channel tag** (`[0x04][id][mime][bytes]`), avoiding
base64's 33% tax. The raw Terminal channel exists because a PTY is a
*continuous, high-volume* stream where the tax is paid per keystroke forever
(`protocol.rs`: "base64-in-JSON would bloat it"). An image is one bounded,
operator-triggered read of at most 4 MiB (§4) — a different cost class. Paying
~1.3 MB of encoding on a deliberate click, once, buys a reply that rides the
existing request/response correlation with zero new client plumbing.

### 3. The extension proposes, the magic bytes dispose

The served media types are a **closed allowlist**: `image/png`, `image/jpeg`,
`image/gif`, `image/webp`, `image/bmp`, `image/x-icon`, `image/svg+xml`. The
path's extension selects a candidate type; the file's **leading bytes must then
agree** with it, or the read is refused as `not an image`.

The daemon therefore never labels bytes with a type it did not verify. Without
the magic check, `notes.png` containing HTML would be handed to the browser as
`image/png` on the operator's say-so — harmless in an `<img>` today, and exactly
the kind of unverified claim that stops being harmless the moment someone routes
these bytes somewhere else. SVG, being text, is checked structurally (a UTF-8
document whose first element is `<svg`, after an optional XML declaration or
comment).

**SVG is served, and only ever inside `<img>`.** In that context every browser
runs SVG script-inert and blocks its external fetches; the §2 rejection of an
HTTP route is what guarantees the context, since there is no URL at which the
SVG could be opened as a document instead. An SVG in a repo is usually a logo or
a diagram, and refusing the one vector format in a developer tool to avoid a
risk that the transport already forecloses would be superstition, not caution.

### 4. A separate, larger cap

`MAX_IMAGE_BYTES` is 4 MiB, distinct from `MAX_READ_BYTES`'s 2 MiB for text. The
text cap answers "how much can an editor usefully hold"; an image's cost is
decode and paint, not lines, and a 3 MiB screenshot is an ordinary artifact. The
refusal reason stays the existing `too large` — the wire vocabulary
(`binary` / `too large` / `not found`) grows by exactly one literal,
`not an image`.

### 5. Markdown resolves its own relative image sources

The preview rewrites each `<img>` whose `src` is repo-relative into a `data:`
URL fetched through `file.image`, resolved against the document's own directory.
An absolute or `http(s)` source is left exactly as it is — that is the author's
explicit request for a remote asset, and it is already what the preview does
today.

This runs **after** `DOMPurify.sanitize`, on the sanitized DOM, so no attribute
we set can re-enter the sanitizer's decision. A source that refuses (missing,
too large, not an image) leaves the element alone: a broken image is an honest
rendering of a broken link, and substituting a placeholder would fabricate.

### 6. What this does not build

No thumbnails in the file tree (a listing that reads every file's bytes is not
the Observe class's shape). No image editing, no Save — the viewer is read-only,
so the Write class (ADR-0036's amendment) is untouched. No video, audio, or PDF
viewer; those keep refusing. No caching layer — a click is a read, and the
watcher's existing nudge is what makes a re-read correct.

## Consequences

- **The verb registry gains one Observe row** (`file.image`). Like its siblings
  it answers on the requesting id, never spawns, and never consults the run lock.
- **Confinement is unchanged.** The new reader calls the same `confine` kernel,
  and inherits the escape→`not found` masking of ADR-0036 §5 — an out-of-root
  image read is indistinguishable from a miss, exactly like a text read.
- **The refusal vocabulary grows by one literal** (`not an image`), which the
  browser flashes like any other reason. `binary` now means "binary and not an
  image we serve".
- **The client-side `binary` refusal now flashes too.** It previously emitted
  only the seam event, so clicking a `.pdf` did nothing visible — indistinguishable
  from a broken tree. Narrowing that branch to non-image binaries made the
  silence conspicuous enough to fix; every other refusal on this path already
  reached the operator.
- **The tree's image icon stops lying.** `BINARY_EXT` in the client sheds the
  image extensions to a new `IMAGE_EXT`; `classify()` gains an `image` kind and
  the viewer module a fourth pane alongside code, markdown, and diff.
- **The static `file://` demo** synthesises a placeholder SVG for an image tab,
  the same way it synthesises source text — the demo keeps demonstrating the
  pane without a daemon.
- **No new dependency.** Base64 is `data-encoding`, already in the daemon's tree
  for the TOTP secret.
