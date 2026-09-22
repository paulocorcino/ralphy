# Notes on the stage: a markdown document in an opaque file, shown as a card

Status: proposed (2026-09-22).

The **Consoles tab** is a plane of console windows organised by fences
([ADR-0051](./0051-consoles-stage-plane-and-fences.md)). Everything on it is a
process. There is nowhere to write "before merging #421, run the migration on
the WSL peer" next to the console that will do it — the operator keeps that in
a text editor, a chat window, or their head. The request that opened this
track asked for a post-it on the canvas: markdown, links, a colour, a jump
target because the plane scrolls, saved as its own file somewhere the operator
can back up, and moving with its fence when the fence is detached.

Grilled to its requirements, the post-it is a **document**: it has a file, a
backup, a title, links, headings worth jumping to. A post-it does not need a
backup. So this ADR separates the two things the request bundled — the
**note** (a document on disk) and the **card** (how a note is shown on the
stage) — and decides each. The editor was chosen by measurement, not
preference (§6; the [ADR-0048](./0048-whiteboard-vendored-excalidraw.md)
lesson), and the file format is deliberately *opaque, not secret* (§3).

This ADR amends [ADR-0050](./0050-desk-layout-is-daemon-state.md) (the desk
grows a third collection) and adds two verb-registry rows to
[ADR-0036](./0036-workbench-daemon-integration-protocol.md)'s Write and Read
classes, in the shape [ADR-0055](./0055-console-image-paste-write-verb.md)
established for a daemon-named write. It touches
[ADR-0057](./0057-the-workbench-asset-contract.md) once: a vendored asset that
is *built* rather than copied (§6).

## Decision

### 1. Vocabulary: a note is the document, a card is its view

A **note** is a markdown document owned by the operator, stored as one file
inside a checkout. A **card** is the note rendered on the stage: a rect, in a
fence or not, always editable. Closing a card never touches the note; a note
with no card is still a note. Both words enter [CONTEXT.md](../../CONTEXT.md).

*Review notes* ([ADR-0061](./0061-review-notes-on-a-diff.md)) are a different
noun: annotations *about* a diff, kept in the desk document until sent, never
written into the repo. The glossary says so in one line, and "note" alone
always means this ADR's document.

### 2. The desk grows `notes`, and the record is placement only

`DeskStore` gains an additive `notes` field beside `windows` and `fences`
([ADR-0051 §10](./0051-consoles-stage-plane-and-fences.md) is the precedent:
an existing `desk.toml` keeps loading). A note record is

```
{ id, checkout, path, rect, locked }
```

— a stable client-side id, the checkout the file lives in (as a window record
carries its checkout since [ADR-0063](./0063-a-worktree-is-a-console-workspace.md)),
the file's path relative to that checkout, the rect in stage pixels, and the
lock flag (ADR-0050's lock amendment applies as-is). **Nothing about the
content is desk state** — not the title, not the colour, not the text. The
desk knows *where* a note is shown; the file *is* the note. This is the
load-bearing split: a desk can be deleted and no note is lost; a repo can be
cloned and every note comes along.

The daemon enforces a cap of **32 notes** and the same `rect_is_sane`
rejection as windows. `PUT /api/desk` folds `notes` like the other two
collections (ADR-0050, 2026-09-20 amendment). The detach popup (#344) holds
cards the way it holds windows: a snapshot, never a writer of the desk.

### 3. The file: `.note`, an opaque container around markdown

A note is one file with the extension `.note`:

```
"RNOT"  version u8 = 1  raw-deflate(markdown)
```

The markdown inside starts with a YAML front-matter block that carries exactly
one field today — `color` (§8) — and the body is CommonMark + GFM (task lists,
tables). The title is the first `#` heading; there is no separate title field,
so the document has one source of truth for its name.

**Why a container at all.** The operator asked that agents not read a note by
accident: a note may say "rotate the staging token". A text file in the tree
is exactly what `grep` and `read_file` ingest as context. Raw deflate behind a
private magic gives the effect for free: `file` says "data", `strings` finds
nothing, `unzip`/`gunzip`/`tar` all fail, a vendor's `read_file` returns
noise. No key, no ceremony.

**What it is, in the words the ADR wants repeated: opaque, not secret.** An
agent that runs a shell and *decides* to read a note can — the format is in
this repository. The container prevents an agent from *stumbling* on the
content, which is the stated need; it does not prevent one from *wanting* it.
Anyone who reads "closed format" as "encrypted" should be corrected to this
section. A `file.read` of a `.note` by the workbench's own viewer shows bytes,
as it would for any binary; §11 routes the double-click elsewhere.

**Why not a zip** (the first proposal, "like a docx"). A docx is a zip because
Word, LibreOffice and Google Docs must open one file; nobody but Ralphy opens
a `.note`, so the interoperability that justifies zip is absent — and zip is
the most inviting shape possible for a curious agent: `PK` magic, `file`
answers "Zip archive", `unzip -p` prints the note. A 300-byte note also
deflates to more than 300 bytes; compression is not the point, and the entry
would have been *stored* anyway.

**Why not encryption.** The question "where does the key live" is a security
decision — in `RALPHY_DAEMON_DIR` it is plaintext at rest; as a passphrase it
is a prompt per session — and the operator's actual need was "not read by
accident", which needs no key. The container is the seam: if a secret note is
ever wanted, an encrypted entry is a version bump of this format, not a new
one.

**Cost, accepted and recorded: git sees a binary.** No diff in a PR, no
`blame`, an unmergeable conflict (pick mine or theirs), a whole blob per
edit. For a note this is acceptable; for a document it would not be, which is
one more reason a note is not the place for a runbook.

### 4. Where a note lands, and what identifies it

The default directory is **`.ralphy/notes/`** in the checkout — run state,
gitignored once `ralphy` has run in the repo (`ralphy_core::gitignore`), never
in the Change set, listed by the explorer (ADR-0036's tree deliberately
ignores `.gitignore`). The operator may choose another directory **inside the
checkout** at creation (§9) — "I want this backed up with the repo" is a
legitimate reason, and it is a choice of *place*, not of format. Outside the
checkout is refused by the verb (§5).

**The file name** is set at the first autosave from the first `#` heading at
that moment — `deploy-staging.note`, an ASCII slug, `-2` on collision — or
`note-<UTC yyyymmdd-hhmmss>.note` when there is no heading yet. After that the
name **never changes by itself**: retitling does not rename, because a silent
rename breaks another card's link, the operator's backup and the desk record
at once. Renaming is an explicit action in the card's menu that goes through
the existing `file.rename` and updates `path` on the record.

**Identity is `(checkout, path)`.** There is no `id` in the front-matter — the
magic already answers "is this ours", and a copied file *is* a second note.
Two worktrees of one project (ADR-0063) with the same relative path hold two
notes; the card shows the checkout name in its head when the desk has more
than one, the way the fence list shows "repos contained". Notes do not
synchronise across worktrees; one that should be shared goes into a committed
directory and merges like any file.

### 5. Two verbs with a verb-fixed target class: `note.read`, `note.write`

The registry gains one Read row and one Write row. Both take a client-named
path, and both **require the `.note` extension** and confine to the checkout
through the same kernel every client-named write uses (`confine_write`,
`PROTECTED_DIRS`). The difference from `file.read`/`file.write` is what the
bytes are:

- `note.read { path }` → `{ markdown }` or a refusal `not a note` (bad magic
  or version) — the codec lives in the daemon, and the browser never sees the
  container.
- `note.write { path, markdown }` → encodes and overwrites. **No base hash**:
  last writer wins, exactly as `file.write` does for the Changes panel today
  (§7 owns the consequence).

`PROTECTED_DIRS` *contains* `.ralphy` (`fswrite.rs`: `.git` and `.ralphy`,
refused on the spelling sent and on the path it resolves to), which is where
the default directory lives. So the confinement kernel gains **one carve-out,
one extension wide, in one directory**: a path whose final component ends in
`.note` and whose parent resolves to `<root>/.ralphy/notes/` is not protected.
The carve-out is in the kernel, not in the note verbs, on purpose — the
explicit rename and delete of §11 go through the existing `file.rename` and
`file.delete`, and they must reach the same file. Everything else under
`.ralphy/` (and all of `.git/`) stays refused to every client-named write; a
`file.write` that lands bytes in `.ralphy/notes/x.note` writes a file only
`note.read` will ever look at, and it will answer `not a note`. This is the
[ADR-0055 §3](./0055-console-image-paste-write-verb.md) discipline — the
denylist is not opened for the client's convenience — with the one hole
measured to the feature that needs it.

No transport is added; the fleet ([ADR-0052](./0052-local-fleet-federation.md))
carries the verbs to a peer's checkout unchanged.

### 6. The editor is hybrid WYSIWYG: Milkdown Crepe, lean, vendored as a build

The operator wants the Notion reading experience: no source mode and no
preview mode, `## ` becomes a heading as it is typed, a task list has real
checkboxes, a table is a grid. That rules out the workbench's existing
markdown path (marked + a textarea, or Monaco: Monaco's decorations style
text but cannot hide it, so a hybrid cannot be built on it) and asks for a
second editor engine on the page. Three were tried in a spike on 2026-09-22
(Playwright 1.62, headless Chromium, the repo's own Monaco AMD loader on the
same page, a fixture with headings, task list, table, links, a mermaid fence,
code, quote and an ordered list):

| | Milkdown Crepe 7.22.1, lean | ink-mde 0.34.0 | Editor.js |
|---|---|---|---|
| family | document model (ProseMirror), markdown serialised in/out | source-based live preview (CodeMirror 6) | block editor |
| stores | markdown | markdown | **its own JSON** — rejected on that alone |
| last release | 2026-08-12, monthly cadence | 2024-09-28 | — |
| bundle, esbuild 0.28.2 minified | **716 KB JS + 21 KB CSS, one file each** (237 KB gz) | 646 KB eager over 12 chunks; 2.0 MB as one file | — |
| coexists with Monaco's AMD loader | yes, no errors, Monaco models intact | yes | — |
| mount, 10 cards of 240×180 | 95 ms (~10 ms each), 33 MB heap for the whole page | 22 ms | — |
| fixture roundtrip | bullets `*`→`-` fixed by `remarkStringifyOptionsCtx`; table cells re-padded, one trailing newline — semantic-preserving | byte-identical | — |
| typing `## `, `- [ ] ` | markup dissolves; `/` menu; block handle; selection toolbar; tables edit as grids | markup stays visible (dimmed); **a table never renders as a grid** | — |

Crepe is the one that *is* the requested experience; ink-mde's family cannot
render a table and the project is stale. **Milkdown Crepe, MIT, is adopted**,
built lean through `CrepeBuilder` with seven features — block-edit, cursor,
link-tooltip, list-item, placeholder, table, toolbar — and without the ones a
note does not use: CodeMirror (code-block editing, −1.2 MB of language packs),
LaTeX (−263 KB JS and −1.4 MB of katex CSS), image-block, top-bar, AI. Its
theme is CSS variables (`--crepe-*`), so [ADR-0035](./0035-daemon-ui-visual-language.md)'s
tokens and §2b reading typography map onto it without patching the bundle;
the default theme's inner padding is overridden for the card.

**Two costs, named.** Crepe carries `@vue/runtime-core` (37 KB) for its own
components — the framework tax ADR-0048 refused at 18 MB is here at 37 KB and
accepted. And the first autosave of an existing note normalises table
whitespace and the trailing newline once; a cosmetic diff, never a content
change.

**How it enters the tree — the ADR-0057 exception.** Every other vendor is a
distribution copied as shipped; the lean Crepe does not exist as one. It is
committed as `assets/ui/vendor/crepe/crepe.js` + `crepe.css` *and* the recipe
that produced them: `vendor/crepe/build/` with `package.json` and
`package-lock.json` (pinned `@milkdown/crepe`, `esbuild`), `entry.js` (the
feature and CSS list — *what "lean" means*), `build.mjs`, and Milkdown's
`LICENSE`. Regenerating is `npm ci && node build.mjs` in that directory; it
runs in no CI job and no `cargo build` (ADR-0057 D3: the gate needs no `npm
install`). `build.mjs` writes a one-line header into `crepe.js` — package
version, esbuild version, feature list — and a Rust test pins it, so the
artefact states what it was built from and a rebuild with a different recipe
reds.

### 7. Saving: autosave, one writer, no live sync

There is no save moment in a hybrid editor, so the note **autosaves**: a
debounce of ~800 ms after the last change, plus a flush on the card losing
focus, on `beforeunload`, and on `Ctrl+S` (which costs nothing and answers a
reflex). The footer dot is lit from the change until the verb acknowledges;
a write error turns the card border `danger`, keeps the text in the editor,
and the next debounce retries. `note.write` overwrites — **last writer wins**,
the rule the Changes panel already lives by.

Two clients showing the same card (ADR-0051 §9: the desk is shared) do **not**
sync content live. A card reads the note when it mounts, and re-reads it when
it wakes from dormancy (§14) or gains focus *with no pending change*. If both
sides edit, the later `note.write` wins; that is a known limit, recorded here
rather than solved — pushing bytes into a ProseMirror with a live cursor is a
merge problem, and the desk's philosophy is already "one writer per session".
CRDT/OT is not built.

### 8. The card on the stage

- **Size.** Default 240×180, minimum 160×100, no maximum: a card is a rect on
  the plane and resizes with the window grips. The body scrolls inside; a
  long note is a note with a big card. Navigation inside a long note is the
  anchor map (§10), not a "maximize".
- **Colour.** A closed palette of six desaturated tones over ADR-0035's ramp —
  `ochre`, `sage`, `rose`, `slate`, `plum`, `sand` (default) — an enum in the
  front-matter, hex only in the stylesheet. Rendered as a 3 px top band and a
  ~6 % tint of the body ground, not a saturated square; the same tone marks
  the note in the map. Changing it rewrites the file (it is note state).
- **Focus** as for windows: `border-focus`, raised in the stack. No shadow.
- **Membership** is derived by centre point like a window's (ADR-0051 §6): the
  card moves with its fence and rides into the detach popup with the snapshot.
  A note may sit outside any fence.
- **Arrange ignores notes.** Arrange tiles consoles into a grid; a note placed
  beside a console stays beside it.
- **Lock** (ADR-0050 amendment): a locked fence or a locked note neither moves
  nor resizes, **and** the editor goes read-only (`setReadonly(true)`).
- **Head:** grab, title (from the first heading, "Note" when none), colour
  dot, close. **Footer:** the path, dim; the unsaved dot.

### 9. Creation has no dialog

`Note` is one toolbar control in the shape `Fence` already has (ADR-0051 §7
amendment): its first row creates, the rest is the map (§10). "New note" puts
a card at the viewport centre, already in edit with the cursor placed; the
footer shows `.ralphy/notes/` as an **editable directory field** until the
first autosave, after which it is read-only and a move is the explicit rename.
The first autosave fires only once there is content, so changing the
directory before typing never leaves an empty file in the wrong place. A
directory outside the checkout is refused by `note.write`; the footer turns
`danger` and the text stays in the editor.

### 10. Navigation: the map, and `##` as anchors

The `Note` menu lists every card on the desk — colour dot, title, the fence
it sits in or "loose" — and clicking one **centres** it (a note is a point of
interest, like a window in the Go-to picker; a fence is a region and anchors
its corner). Each row expands, collapsed by default, into the note's `##`
headings; clicking one jumps to the card *and* scrolls its body to that
heading with a brief highlight. `#` is the title and `###` is noise at card
size; only `##` are anchors. The map is derived from what the tab has already
loaded to render its cards — the daemon knows no headings.

### 11. Closing, deleting, reopening, and the missing file

- **Close** (the card's `✕`) removes the record from the desk and **keeps the
  file**; a toast says so, with an undo.
- **Delete the file** is a separate menu action with a confirmation naming the
  path; it goes through `file.delete` and drops the card in the same gesture.
- **Reopen** is a double-click on a `.note` in the explorer. The explorer
  routes the extension to "open as note": if a card for `(checkout, path)`
  exists it jumps to it; otherwise a card is born at the viewport centre,
  outside any fence, with the file's colour. `note.read` validates the magic;
  a file that is not ours opens in the viewer as bytes with the refusal
  shown. This is also the portability answer: a cloned repo, or a desk that
  was lost, recovers every note through the explorer — no scan-and-offer
  feature is needed.
- **Missing file** (a branch switch, an `rm`): the card dims by opacity
  (ADR-0035 §5), the body reads "file missing: <path>", the tools shrink to
  *remove* and *point elsewhere*. It is never recreated on the operator's
  behalf.

### 12. Links

A relative link resolves against the **checkout root** (the path the explorer
and Changes show, not the note's directory) and opens through the viewer's
ordinary open-a-file flow — code, markdown, image, or a `.note` (§11). An
absolute `http(s)` link opens in a new tab with `rel="noopener"`; any other
scheme is dropped by the DOMPurify pass the viewer already applies. Crepe's
link tooltip opens the raw href by `window.open`; the card intercepts it with
the same delegated listener the viewer's article uses. A link to a fence or a
console is not built.

### 13. Keyboard

Verified in `app.js`: `consoleShortcutsBlocked()` already returns true when
`document.activeElement.isContentEditable`, which a ProseMirror is — every
global accelerator (`Alt+Shift+digit`, `Alt+Shift+←/→`, `Alt+Shift+F<n>`, `/`)
is inert while typing in a card, with no change to that predicate. Two keys
cross the boundary: **`Esc`** leaves the editor for the card (accelerators
return; a second `Esc` blurs the card) and **`Ctrl+S`** flushes (§7). `Tab`
belongs to the editor (list indent) — leaving a card is `Esc`, and that
accessibility exception is written here on purpose.

### 14. Dormancy

The dormant-consoles observer (`wb-console.js`, PR #418: `IntersectionObserver`
over `#workspace`, 300 px margin, 15 s) covers cards too. A card off the
viewport long enough **disposes its editor** and stays as a placeholder — the
colour band and the title, both known without reading the file — and rebuilds
on return with a fresh `note.read`. Two rules on top of the consoles' one: a
card **never sleeps with an autosave pending** (the timer waits for the
flush), and waking is the re-read of §7. The saving is not WebGL contexts this
time (a card has none; ~1 MB of heap and ~10 ms each) — it is that a dormant
card has no pending edit by construction, which is what makes §7's re-read
rule safe.

### 15. Mermaid in the card

A mermaid fence renders in the card as a diagram: a Milkdown node view over
the `code` node with `lang: mermaid`, drawn by the already-vendored
`mermaid.min.js` at `securityLevel: strict`, the viewer's own path. A syntax
error shows the raw source with a `danger` border and never disappears.
Editing is a popover anchored to the block — monospace source on the left,
live preview on the right; `Esc` cancels, `Ctrl+Enter` applies and §7 saves.
This is the one exception to "no modes", and it is local to the block. It is
the **last issue** of the series, after the tracer bullet, not a "not built".

### 16. What this does not build

No `.md` open variant ("agent-readable note" was considered and dropped: two
formats for one noun was confusion, and a note the agent should read is a
file the operator writes as a file). No encryption (§3). No "open in the
viewer" for a card — that would be two Crepe instances autosaving one file
(§7); if wanted later, the viewer opens a note read-only while its card
exists. No live content sync, no CRDT. No notes sidebar or list beyond the
`Note` menu. No clickable task checkboxes that rewrite the file. No link to a
fence or console. No keyboard accelerator for "new note". No touch-specific
behaviour beyond what the plane gives windows. No scan of `.ralphy/notes/`
(§11 makes it unnecessary).

## Rejected alternatives

- **Editor.js** — stores its own JSON blocks; markdown conversion is a lossy
  third-party plugin. It would have made the `.note` a container around a
  library's format. Also one package per block type, against ADR-0057's
  vendoring.
- **ink-mde / a CodeMirror 6 live preview** — the Obsidian family: markup
  stays visible, a table never renders, last release 2024-09. Measured in the
  same spike (§6 table).
- **marked + textarea with a raw/preview toggle** — the workbench already has
  it and it is a mode switch, which is what the operator asked to remove. It
  remains the fallback if the vendored bundle ever has to go.
- **A note as a window kind** — a window record is a session's placement; a
  note has no session, no agent, no maximized state. A third collection is
  smaller than a `kind` on the first.
- **Content in the desk** — would have made the note daemon state with a
  browser-only editor, un-backupable, and gone with the desk.
- **Colour in the desk** — was the first draft; moved to the file when
  close/reopen (§11) showed a reopened note would lose it.
- **`docs/ralphy_notes/` as the default** (the request's default) — a
  committable default puts binary blobs into the working tree by default;
  `.ralphy/notes/` is run state, and the operator opts *into* the tree.
- **Zip container, encryption, XOR "obfuscation" with a key in the binary** —
  §3. The last is security theatre: it hides nothing from anyone who looks and
  suggests a secret where there is only opacity.
- **Excalidraw-style whiteboard** — ADR-0048 stands; a note is text.

## Consequences

- **ADR-0050's desk shape grows a third collection.** Additive; an old
  `desk.toml` loads. The wire body of `PUT /api/desk` gains `notes`; the fold
  and the cap follow the fence precedent.
- **ADR-0036's registry gains two rows** (`note.read`, `note.write`), Read and
  Write classes, answering on the requesting id, never spawning, never
  consulting the run lock. The `.note`-under-`.ralphy/notes/` carve-out in
  the confinement kernel is the one hole, one extension wide (§5), and it is
  tested the way `PROTECTED_DIRS` is: on the spelling and on the resolved path.
- **A `note` codec module in the daemon** (`note.rs`): magic, version, raw
  deflate via `flate2` (already in the workspace), front-matter read/write for
  `color`. Unit-tested for roundtrip, bad magic, bad version, and that the
  bytes contain no plaintext.
- **`vendor/crepe/` is the first built vendor asset** and carries its recipe
  (§6). The tag cross-check of ADR-0057 D1 covers its two `<link>`/`<script>`
  tags; the header pin covers its provenance.
- **`detached-fence.html` loads `crepe.js`/`crepe.css`** — ~740 KB more in the
  popup, as it already pays for xterm.
- **`.ralphy/notes/` is run state**: explorer shows it, Changes never lists
  it, `git` ignores it after the first `ralphy` run in the repo (the daemon
  does not write `.gitignore` — ADR-0036 §3).
- **The viewer learns one extension.** A `.note` double-click routes to "open
  as note" (§11); nothing else in the viewer changes.
- **A changelog fragment per PR**, `feature`.

## Implementation notes (not decisions)

The series, each a vertical slice:

1. **`.note` codec + verbs** — `note.rs`, `note.read`/`note.write`,
   confinement and the `.note`-under-`.ralphy` exception, registry tests.
2. **Card on the desk** — `notes` record, cap, fold, render on the plane
   (drag, resize, focus, colour band), membership/detach, lock, close with
   undo, `detached-fence.html`.
3. **Editor** — `vendor/crepe/` with its `build/`, header pin, the card mounts
   Crepe, theme variables, autosave and flush rules, footer path field,
   creation flow, links, `Esc`/`Ctrl+S`, read-only under lock, dormancy.
4. **Map** — `Note` menu (create + map), `##` anchors, jump and scroll.
5. **Explorer** — double-click routing, jump-or-create, missing-file state,
   rename and delete actions.
6. **Mermaid** — node view + popover (§15).

The spike that produced §6's numbers is recorded in
[docs/spike-note-editor-2026-09-22.md](../spike-note-editor-2026-09-22.md);
the page, bundles and screenshots were scratch and were not kept.

## Amendment (2026-09-22): the container carries a length and a mask, because deflate alone did not keep §3's promise

Implementing §3 measured two things the sketch did not survive. The container
is now

```
"RNOT"  version u8 = 1  len u32le  mask(raw-deflate(markdown))
```

**A `len` field, because a truncated note read as a short note.** `flate2`
answers a stream that ends mid-block with the bytes it managed and *no error*:
a half-written or clipped file inflated to a shorter markdown, the card showed
it, and the next autosave wrote that back. Corruption turning into silent data
loss is not a cost this ADR accepted. `len` is the inflated byte length and
`decode` refuses a stream that does not produce exactly it, so the container is
its own integrity check. It also bounds the inflate allocation, which is where
the decompression-bomb refusal moved.

**An XOR mask, because deflate does not always compress.** §3 promises
"`strings` finds nothing". A short or incompressible text — a three-line note,
which is the common case — is emitted by deflate as a STORED block: the
markdown in the clear behind a five-byte header. The first test written for
this section failed on exactly that. The payload is therefore XORed with a
fixed eight-byte pattern, published in `note.rs`.

The mask changes nothing about what §3 decided and everything about whether it
is true. It is **not** a cipher: the pattern is a constant in this repository,
there is no key, and "opaque, not secret" is still the whole claim — it is the
"small blur" the operator asked for in the conversation that produced this ADR.
Encryption remains a version bump, as §3 already says.

`Compression::best()` replaces the default level for the same reason: a note is
small, the cost is microseconds, and fewer stored blocks is the point.

## Amendment (2026-09-22): the recipe lives outside the embedded tree, the record names its project, and two limits found while building it

Four corrections from implementing §§2, 5 and 6. None changes a decision; each
says what the decision costs in this codebase.

**The Crepe recipe is `crates/ralphy-daemon/vendor-build/crepe/`, not
`assets/ui/vendor/crepe/build/`.** §6 put the build script beside the artefact,
in the shape ADR-0057 asks for. But `src/lib.rs` embeds `assets/ui/` wholesale
with `include_dir!` and the router serves every path under it, with no exclusion
mechanism: a `build/` there would put `package.json`, `node_modules/` and a
README inside the binary and on the wire. The recipe therefore sits beside
`ui-tests/`, which is outside the embedded tree for the same reason. The
artefacts (`crepe.js`, `crepe.css`, `LICENSE`) are where §6 said, and the
provenance header they carry is pinned against the recipe by a Rust test — so
"the recipe is committed beside the artefact it built" still holds, one
directory over.

**A note record carries `repo`.** §2 wrote the record as
`{id, checkout, path, rect, locked}`. The desk is one plane across every
registered project and the note verbs take a `repo` like every other verb, so
`(checkout, path)` alone does not say which tree `path` is relative to.
`DeskNote` therefore has `repo`, identity is `(repo, checkout, path)`, and a
re-key rewrites it exactly as it rewrites a window's.

**`note.write` crosses the worktree gate; `file.rename`/`file.delete` do not.**
§5 asks for a carve-out so the generic byte-ops reach a note, and §11 for a
rename and a delete in the card's menu. Both hold in the primary tree. In a
worktree only `note.write` passes: it resolves the worktree's own root the way
`spawn_cwd` does, while every other Write verb is still refused there
(ADR-0063 §2, "lifted in a later slice"). Renaming or deleting a note that
lives in a worktree is therefore not in v1, and the card hides both actions
there rather than offering a refusal.

**The editor's slash menu is clipped by a small card.** Measured: the menu
mounts inside the editor's own element and is ~480px tall, and `blockEdit`'s
`root` options are floating-ui *boundaries*, not portals — pointing them at
`document.body` moves nothing. A card therefore opens at 320×260 instead of the
sketch's smaller box, and the answer to a cramped menu is to resize the card.
Every block the menu offers is also reachable by typing it (`# `, `- [ ] `,
`|`), which is the path §6 chose this editor for.
