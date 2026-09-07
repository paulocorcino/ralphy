# The console accepts a pasted image: one Write verb, daemon-named bytes, the path is what gets pasted

Status: accepted (2026-09-07).

Pasting a screenshot into a workbench console does nothing. xterm.js hands the
console only the clipboard's `text/plain`, the session codec carries UTF-8 text
(`encodeTerminal` in `wb-console.js`), and no verb writes bytes — an image on
the clipboard has no text representation, so the paste event produces no data
and the operator sees nothing happen.

The agent CLIs on the other side of the PTY cannot take the bytes either. No
CLI reads an image from stdin; a terminal graphics protocol (sixel, kitty,
iTerm2) is output-only. Claude Code alone reads the **OS clipboard** on its own
paste key (`Alt+V` on Windows), and it reads the clipboard of the machine it
runs on — the daemon host — which is the operator's machine only in the local
case and never through dev tunnels or a WSL peer. Codex and Gemini have no
clipboard-image path at all: their input layer accepts text and **file
paths**. A file on disk plus its path in the prompt is the one contract every
vendor honours, and it is what the community's bridge tools do by hand.

This ADR decides how the console gets a pasted image onto the daemon's disk and
the path into the prompt. It **amends
[ADR-0036](./0036-workbench-daemon-integration-protocol.md)** — the Write
class of its 2026-07-13 amendment gains one row — and is the write-direction
mirror of [ADR-0049](./0049-workbench-serves-image-bytes.md). Confinement (§5)
is not reopened.

## Decision

### 1. A registry row, not a route: `image.write`

> **`image.write`** (Write) — accept base64-encoded bytes, verify they are an
> allowlisted raster image, write them to a file the **daemon names** under a
> directory the **verb fixes**, and answer with the repo-relative path.

The payload is `{ repo, base64 }` and nothing else. The client sends no path,
no filename and no media type: there is no client-named target for confinement
to check, and no client claim about the bytes for the daemon to trust. Like
`plan.discard`, the verb fixes its own target; a `path` in the payload is
ignored, not honoured.

**Rejected: a raw HTTP upload route** (`POST /api/repos/{slug}/upload`). It is
the obvious design, and ADR-0049 §2 already killed it for the read direction
for reasons that hold unchanged here: capabilities are rows in the verb
registry, not routes; a second door re-implements registry lookup, confinement
and the auth gate somewhere that must now be kept in step with the first; and a
verb reply rides the socket the client already correlates on, so a refusal
arrives as a reason string the console can print, not an HTTP status a paste
handler has to translate.

### 2. The bytes dispose; there is no extension to propose

ADR-0049 §3 has the extension propose a type and the magic bytes dispose. A
paste has no extension. The daemon therefore **sniffs the leading bytes** against
a closed allowlist and refuses anything that matches none of it as
`not an image` — the same literal the read direction uses.

The write allowlist is **raster only: PNG, JPEG, GIF, WebP.** It is narrower
than the read allowlist on purpose:

- **SVG is out.** It is script-bearing text. The read direction serves it only
  because `<img>` is script-inert and there is no URL to open it at; the write
  direction would put an operator-pasted SVG *into the working tree*, where the
  next `file.image`, the next markdown preview, and the next commit all see it.
  A screenshot is raster; nothing is lost.
- **BMP and ICO are out.** No browser paste event produces them: a Windows
  Print Screen lands in the OS clipboard as a DIB, and the browser normalizes
  it to PNG before the page ever sees it. The browser is the format normalizer,
  which is why the BMP-on-WSL bug Claude Code had to fix cannot happen here.

The media type the daemon derives from the sniff also picks the file's
extension, so the name on disk is never a claim the bytes do not back.

### 3. The landing directory: visible, never gitignored, named by the verb

The drop lands in **`.ralphy-clipboard/`** at the repo root, created on first
use, as `paste-<UTC yyyymmdd-hhmmss-mmm>.<ext>` (with a `-N` suffix if that
name already exists). The file is created with `create_new` — an `image.write`
never overwrites anything. The reply is
`{ status: "ok", path: ".ralphy-clipboard/paste-….png" }`, forward slashes on
every host.

**Rejected: landing in `.ralphy/`.** Two independent reasons:

- `.ralphy` is in the Write class's `PROTECTED_DIRS` denylist — it is
  daemon-and-run state the daemon reads back as trusted config, and opening a
  hole in that list for one verb is exactly what the narrow `plan.discard` verb
  exists to avoid.
- It is gitignored, and **Gemini refuses to read gitignored files** (issue
  #275, observed live). The path's *spelling* does not matter — absolute or
  relative, `read_file` resolves the file and applies the gitignore filter to
  it. The escape hatch (`fileFiltering.respectGitIgnore: false`) is a field of
  open upstream bugs (several tools hard-code `true`; negation in
  `.geminiignore` does not un-ignore), and it would change Gemini's behaviour
  for the whole repo to serve one directory. A feature that works for two
  vendors and fails silently on the third is the worst kind of failure: the
  operator pasted, saw the path appear, and the agent answers that it cannot
  read the file.

**Rejected: landing outside the repo** (the daemon's state dir, the OS temp
dir). Vendors sandbox reads to the working tree to varying degrees; a path the
agent may not open is the same silent failure as above.

The visible directory has a cost, accepted deliberately: it appears in the
explorer and the Changes panel reports it, so an operator can commit a
screenshot by accident. That is the honest rendering of what happened — bytes
were written into the working tree — and the Changes panel exists to show
exactly that. Cleanup is the operator's (or the agent's) in this slice; see §7.

### 4. One cap, not two

The decoded size is capped at `MAX_IMAGE_BYTES` — the 4 MiB ADR-0049 §4 set
for the read direction — rather than a new number. Whatever the console pastes,
the workbench viewer must be able to display back through `file.image`; a
second cap would make that a coincidence. The base64 *string* is bounded before
decoding (`4 × cap / 3 + 4`), so an oversized payload is refused without ever
allocating its decode. The refusal is the existing `too large`.

### 5. The path is what gets pasted

On an `ok` reply the console pastes the **path** into the terminal through
xterm's own paste path (`term.paste`), so it arrives bracketed when the child
enabled bracketed paste and as plain text otherwise. Either way the injected
text carries **no trailing newline** — the rule the OSC 52 handler's
`scrubClipboard` already enforces in the other direction: a paste must never
become an execution. The operator reads the path in the prompt and presses
Enter, or does not.

A **watcher's paste is refused by the client** with the same `watching` gate
and the same visible flash as its keystrokes (issue #335): the drop is a write
into the writer's repo, and a client that did not claim the writer slot does
not get to make it from the paste handler either. A paste that carries no image
falls through to xterm's default text paste untouched.

### 6. No new transport, and the fleet for free

The verb rides `/ws/command` like every Write row — one command, one reply on
the id, no new socket, no new frame tag. That socket's handler already routes
every verb by the repo ref's routing head to the daemon that owns the repo
([ADR-0052](./0052-local-fleet-federation.md)), so a paste into a WSL peer's
console lands in the peer's repo with no code written for it. A peer on an
older build answers `unknown verb`, which the console prints as a refusal.

The 33% base64 tax is the same trade ADR-0049 §2 made: one bounded,
operator-triggered transfer of at most 4 MiB, not a continuous stream.

### 7. What this does not build

No generic file upload — drag-and-drop of arbitrary files is a different
feature with a different size class and a different transport question, and it
would need its own decision. No automatic cleanup or TTL for
`.ralphy-clipboard/` — the directory is ordinary working-tree content the
operator owns; a sweep is a later decision if the noise proves real. No
clipboard bridge to the daemon host (writing the browser's image into the host
OS clipboard so a CLI's native paste finds it): only one vendor could use it,
it would give the daemon an OS-clipboard write surface the OSC 52 handler was
built to keep narrow, and on a local daemon it would clobber the operator's own
clipboard. No non-raster types.

## Consequences

- **The verb registry gains one Write row** (`image.write`). Like its
  siblings it answers on the requesting id, never spawns, and never consults
  the run lock. The `spawn_argv` catch-all and the registry count test are the
  compile-time and test-time gates that it stays a Write.
- **The drop writer is its own module** (`clipboard.rs`), not a `fswrite`
  growth: it is a distinct responsibility — a daemon-named write under a fixed
  directory — and it needs only the public `confine_write` kernel and the
  shared `WriteError`. `PROTECTED_DIRS` is untouched.
- **`.ralphy-clipboard/` is a reserved name.** The tree shows it (the listing
  has no hidden-dir filter), Changes reports it, and nothing in Ralphy ever
  gitignores it. Renaming it — if a vendor's hidden-folder handling turns out
  to need a name without the leading dot — is a one-line amendment here.
- **The refusal vocabulary does not grow.** `not an image` and `too large`
  already exist; the paste handler prints them with the console's own notice
  convention.
- **The detached console popup must load `wb-daemon.js`.** It reuses the
  consoles module but did not carry the verb bridge; without it a paste in the
  popup would be a silent no-op, which is the one outcome this ADR exists to
  remove.
- **No new dependency.** Base64 is `data-encoding`, already in the daemon's
  tree.
