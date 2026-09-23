# Notes on the stage: a markdown document in an opaque file, shown as a card

Status: **accepted** (2026-09-22) — implemented the same day, in the six
slices the implementation notes list, on `feat/notes-on-the-stage`. Three
amendments below record what measurement changed: the container gained a
length and a mask, the Crepe recipe lives outside the embedded tree, and a
note record names its project.

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

**A `.note` that is not a container shows a refusal, it does not open a pane.**
§11 says such a file "opens in the viewer as bytes with the refusal shown". The
viewer serves text and images and refuses everything else, so opening it would
have produced an empty pane carrying the same words. The explorer therefore
says `<path> is not a note` and no card is left behind — the part of the clause
that matters (the operator is told, and no editor is put over bytes it would
overwrite) is kept.

**The editor's slash menu is clipped by a small card.** Measured: the menu
mounts inside the editor's own element and is ~480px tall, and `blockEdit`'s
`root` options are floating-ui *boundaries*, not portals — pointing them at
`document.body` moves nothing. A card therefore opens at 320×260 instead of the
sketch's smaller box, and the answer to a cramped menu is to resize the card.
Every block the menu offers is also reachable by typing it (`# `, `- [ ] `,
`|`), which is the path §6 chose this editor for.

## Amendment (2026-09-22): the card wears the plane's chrome, its title renames it, and the ground and the ink are chosen apart

Three defects the operator found on the first plane with a real note on it.

**A restored card had no place in the window tier.** §8 says a card is focused
"as for windows: raised in the stack". A console gets its `z-index` from
`focusWin`, which `buildChrome` ends in — so every window has one from the
moment it exists, restored or not. A card got one only when it was focused, and
a restore never focuses anything: it came back at `z-index: auto`, **under**
every console (z ≥ 61). The card was visible where nothing overlapped it and
deaf where something did — a click on what looked like its body landed on the
terminal's canvas and the keystrokes went to the shell, which reads exactly
like a note that cannot be typed into. `WBConsole.stackWin` now puts a surface
in the tier without focusing it, and `buildCard` calls it. A surface on the
plane is in the tier or it is under it; there is no third state.

**The card drew its own glyphs.** §8's head listed "grab, title, colour, lock,
close" without saying what they are drawn with, and the first implementation
used text characters (`⠿`, `◑`, `⋯`, `🔓`, `×`) in a bordered box, beside a
console titlebar drawing Bootstrap Icons in a 22 px borderless square. Two
titlebars on one plane that do not look alike read as two applications. The
card's controls are now `bi-grip-vertical`, `bi-palette`, `bi-three-dots`,
`bi-lock-fill`/`bi-unlock` (the console's own two) and `bi-x-lg`, in
`.session-actions button`'s geometry. The one deliberate difference is the
colour, which follows the card's ink rather than `--text-muted` — see below.

**The title is renamed in place, and that is not a file rename.** §4 decides
that the title is the first `#` heading and that retitling never renames the
file. It did not follow that there is a *way* to retitle: a note whose body is
empty has no heading to edit, and the only naming gesture on the card was
`⋯ → Rename file…`, disabled until a file exists — so a fresh card could not
be named at all. Clicking the title now opens a field over it that writes that
heading (inserting one when there is none, removing it when the name is
cleared), through the same scan `titleOf` reads, so the two never disagree
about which line names the note. The file keeps its name; `⋯ → Rename file…`
is still the only thing that moves it.

**The ground and the ink are two choices, not one.** §8 decided a single
`color` rendered as a 3 px band and a ~6 % tint. A tint that quiet cannot be
what anyone means by "a yellow note", and a card that *is* yellow needs a text
colour chosen for it — the theme's `--text` is written for the dark ground. The
front matter therefore carries three fields, each a closed set:

```
color: ochre | sage | rose | slate | plum | sand      the tone
fill:  wash | solid                                   how much of it the ground takes
ink:   default | light | dark | <any tone>            the text over it
```

`fill` and `ink` are **omitted when they are the default**, so a note nobody
restyled keeps the single `color:` line it has always had and no existing file
is rewritten. Both are picked from one popover behind the palette control,
because neither is legible without the other. The chrome (grab, title, tools,
footer) follows the ink at reduced opacity for the same reason. Unknown names
fall back, as a tone always did — a hand-edited file never breaks a card.

Found by driving the whole life of a card in a browser rather than the three
complaints alone: opening the rename on the title's `pointerdown` — and
stopping that press so it could not also arm a drag — left the card movable
only by the 10 px of grip beside the title, because the title is `flex: 1` and
therefore most of the head. Nothing is stopped on the way down; a press that
did not move by the gesture's own 3 px opens the field on the way up.

## Amendment (2026-09-22): the name lives in the header, and the editor is sized for the card

Three more defects from the same plane, all of them the difference between an
editor built for a page and an editor living in a 320 px card.

**The title is a front-matter field, not the body's first heading.** §3 and §4
put the name in the document as the first `#` heading, reasoning that this
gives "one source of truth for the name". It does — and it also prints the
name twice on a surface that already has a titlebar: retitling a card inserted
an `h1` at the top of the body, one line under the identical text in the head.
The operator's words: *deveria ficar apenas no título, não preciso disso no
corpo do notes*. The name moves into the front-matter block beside `color`:

```
title: "Sprint 12: what is left"     the name, a quoted YAML scalar
color: ochre | sage | …              the tone (unchanged)
```

Quoted, because a title with a colon in it is ordinary and a bare scalar would
make the block invalid YAML to anyone else's parser. Omitted when the note has
no name, so an untitled note's header is the single `color:` line it always
was. **One source of truth is preserved** — it is the field now, and the field
wins over any heading — and the body is what the operator wrote and nothing
else.

*Legacy notes are read, never silently rewritten.* Every note written before
this amendment carries its name as the body's first heading, so `titleOf`
falls back to exactly that shape — a `#` on the body's FIRST line, not the old
"first `#` anywhere", because with the title out of the document a `#` further
down is a section the operator wrote. Retitling migrates: the heading is
lifted out of the body and into the header, and the name stops being printed
twice. Nothing else migrates — opening a note writes nothing, and restyling
one carries whatever field is already there without promoting a heading into
it.

**A card's editor is not a page's.** Crepe sizes its floating chrome for a
full-width document: 32 px toolbar buttons around 24 px icons, slash-menu rows
at `min-width: 220px` with 14 px of padding and a 420 px scroller. On the
320 × 260 default card that is a toolbar 264 px wide and a menu taller than the
note it is inserting into. Worse, both draw their glyphs in
`--crepe-color-outline`, which this theme maps to `--border` — a hairline
colour, invisible as an icon. `styles/13-notes.css` now scales the toolbar,
the slash menu and the link tooltip to the card and recolours their glyphs to
`--text`; the rules only shrink and recolour, so a Crepe bump cannot quietly
undo the fit. List markers follow the card's **ink** rather than `--text`,
because they are part of the document.

**The block handle is off.** Upstream parks the `+`/`⠿` pair in the page's
120 px side padding; §8 gives the card's width to the text, so there is no
gutter. Measured in a browser: the pair is laid out ~70 px to the *left* of the
card's own border — clipped away by the card, or drawn over the first words of
a line — and it never follows the block under the pointer. Both of its acts
survive its removal: `/` opens the same menu its `+` does (verified), and a
block is moved by selecting and cutting it. It is hidden in CSS and not
through `blockHandle.shouldShow`, which is in Crepe's config type and read by
no code in `@milkdown/plugin-block`. The menu itself drops `h4`–`h6`, which a
card renders within a tenth of an em of body text; they are still typeable as
`#### `.

**One node view replaced every node view.** The mermaid fence of §15 was
registered as `editorViewOptionsCtx.nodeViews`, and Milkdown builds its view as
`new EditorView(el, { nodeViews: fromEntries(nodeViewCtx), ...options })` — the
spread puts that object last, so it did not merge with the features' node
views, it replaced them. A bullet list drew no bullet and a task list no
checkbox for as long as §15 has existed. The fence is registered in
`nodeViewCtx` now, beside the features' own.

**A click below the last line stopped the typing.** The blank space under a
short note belongs to Crepe's `.milkdown` wrapper, which sits between the
card's body and the editable element — so the guard that read
`ev.target !== body` let those presses through to the default, which blurred
the editor; the keystrokes after them went to `document.body` and were lost
with no sign on the card. The guard now asks the only question that matters —
is the press outside the editable element — and the wrapper is a flex column
so the editable element fills it, which is what puts the caret where the press
landed instead of where it last was.

**Mermaid drew its bomb on the plane.** A fence that does not parse makes
mermaid render its own "syntax error" cartoon into `document.body` — a 200 px
graphic at the bottom-left of the workbench, outside every card, that nothing
on the plane could close. It is identified by the render id and removed by it;
what the operator sees is §15's error box, inside the note, with the source
still in it.

**Enter keeps its meaning.** The operator asked whether Enter could insert a
soft break instead of a paragraph. The jump it produced was upstream's `4px 0`
paragraph padding, not the paragraph: Enter is also what leaves a list, splits
a heading and ends a quote, and a hard break serialises into markdown as a
trailing backslash. So the block stays a block and the padding goes — a new
paragraph reads as the next line, and `Shift+Enter` is still the soft break.

**A field on a card is not a credential.** Renaming a note raised the
browser's *save your password?* prompt, offering the note's title as the
username: every input a page leaves unowned by a `<form>` is grouped with the
login form's password field, and the password was still sitting in it after a
successful login. The card's three fields now carry the documented opt-outs
(`autocomplete="off"` and the two `data-*` ones the third-party managers read),
and the shell drops the code and the password the moment the daemon accepts
them — they are spent, the session is the cookie, and neither is ever replayed.

## Amendment (2026-09-22, third): what the cursor promises, what a heading looks like, and where a password field may exist

The operator's second pass over a real card. Four defects and one rule.

**A password field exists only while its surface is open.** The previous
amendment attributed the browser's *save your password?* prompt to the note's
fields and gave them the documented opt-outs. It kept happening: Chromium
ignores `autocomplete="off"` for save prompts, and the real cause is the other
side of the pair — a `type="password"` left in the document is autofilled by
the manager and then paired with whatever text field is typed into next. The
gate's password input and the four in the settings modal are now inside
`x-if`, so a workbench in normal use has **no password field in the DOM at
all**. That is the rule for any new one: a credential field is rendered by the
surface that asks for it, never merely hidden.

**The cursor promises what the press does.** `text` over the card's title
promised a caret that the click does not place (a rename is a press, and the
field replaces the label); `move` over the head named a gesture the operator
never needed named. Both are the plain pointer now. `text` belongs over the
body — where it is also true of the padding and of the blank space under the
last line, both of which land a caret.

**A heading has to look like one.** Crepe's reset gives every level
`font-weight: 400`; with the card's levels a fifth of an em apart, typing
`## ` changed nothing anyone could see — the marks vanished into the heading
they made and the line looked like the paragraph it had been. The levels are
600 now, and the scale opens a little (h1 1.35em).

**Applying a diagram redrew nothing.** `update()` runs inside the dispatch and
skips its redraw while the popover is open, so `Ctrl+Enter` left the drawing
showing the version before the edit. The popover closes first, then the
transaction lands. And an EMPTY fence — the state every diagram starts in —
says *"Empty diagram — click to write one"* instead of mermaid's "no diagram
type detected", which reads as a failure on a fence that has never been given
a chance.

## Amendment (2026-09-22, fourth): the marks are written down, the index is on the card, and `[ ]` makes a task

The operator's third pass, and the first one that asked *what can I type*. The
answer measured against the real bundle (Playwright over the vendored
`crepe.js`, every row typed with a keyboard): **the flavour is CommonMark +
GFM**, and every mark of it already worked — `*italic*`, `**bold**`,
`` `code` ``, `~~struck~~`, `# `…`### `, `- `, `1. `, `- [ ] `, `> `, `~~~`,
`|3x3| `, `---`, `[text](url)`. Nothing had regressed since the spike. What
was missing was that **none of it is discoverable**: this editor is hybrid
WYSIWYG (§6), so a mark dissolves the instant it is recognised and there is
nothing left on screen to read it off. The `/` menu answers that question for
blocks and nothing answered it for marks.

**The card carries a cheat sheet.** `⋯ → Markdown help` opens a one-screen
table of what to type and what it makes, in the shell's modal classes over
DOM this module builds itself (so it also works in the detached-fence popup,
which has no Alpine). It names the flavour, the two marks that are this card's
own — `## ` is an index anchor, a ```` ```mermaid ```` fence draws — and the
rule a table cannot show: *a block mark fires on the space after it*. A chip
therefore carries no trailing space; a test pins that, because a space that
cannot be seen is worse than the prose that replaces it.

**`[ ]` on a plain line makes a task.** GFM's own rule only sets the
`checked` attribute and requires a list item to already be there
(`wrapInTaskListInputRule`, read and then measured): `- [ ] ` worked and
`[ ] ` alone was escaped into the document as the literal text `\[ ]`. A note
is mostly checklists and the brackets are what an operator reaches for, so
this bundle adds the wrapping half — and the retick that upstream also
refuses, `[x] ` typed into an item that is already a task. It stands down
wherever upstream's rule applies.

**The index is a control on the card.** §10 put the `##` map in the `Note`
menu, which is the right place to find a note and the wrong place to move
inside one you are already reading. The head gains a `☰` beside the palette:
it lists this note's `##` and scrolls the body to one. It does **not** move
the card — the card is already in front of you — and it is hidden outright
when the note has no `##`, so it never opens onto nothing. The list is built
on each open from the live document, because the headings change with every
keystroke.

**An empty heading says what it is for.** Crepe's placeholder carries one text
for every block type (`placeholderConfig` is a single string), and prints it
as `content: attr(data-placeholder)` — so a per-level word is an override of
that `content`, not a second plugin. An empty `##` reads *"Write a title"*.
Upstream skips list items and code blocks, and we keep that. The heading scale
closes a little at the same time (h1 1.22em): the third amendment opened it,
and weight had already done that work.

**New notes and absent fields are two different defaults.** The operator chose
ochre / solid / dark for a new card. `DEFAULT_TONE`/`FILL`/`INK` could not
carry it: they are also what an *absent* front-matter field means, so
redefining them would have repainted every note already written, silently, on
the next read. A new note's dress is its own constant and `withHeader` writes
all three fields out.

**The scrollbar is there and colourless.** It takes its colour while the card
has the caret or the pointer, and loses it otherwise. Through `scrollbar-color`
alone, on the card's ink: the plane's `*` default already sets the width, and a
`::-webkit-scrollbar` rule beside `scrollbar-color` is inert — a design-system
test in `lib.rs` refuses one, and caught this change trying to add four. The
width is deliberately not what varies: hiding the bar reflows the column under
the pointer, which reads as the text jumping sideways every time the mouse
crosses the card.

**`Point elsewhere…` is the missing card's verb, and only its.** §11 gives it
to a card whose file is gone; the menu offered it on every card, beside
`Rename file…`, where it read as a second and cryptic rename — *"I don't know
what this point… is"*. It is hidden unless the card is in that state, and it
is called **`Use another file…`**.

## Amendment (2026-09-22, fifth): a link is typed, a link is the ink, and the footer says when

**Typing a link made no link.** `@milkdown/preset-commonmark` ships an input
rule for an *image* and none for a link — measured: `[here](path)` stayed text
and the autosave escaped it into the file as `\[here]\(path)`. The only doors
were the selection toolbar and the `/` menu, neither of which is what someone
writing markdown reaches for, and the cheat sheet added an amendment ago was
wrong to list it. The bundle adds the rule. It also adds **`@@<path> `**,
which expands to `[<path>](<path>)`: a note about a repo is mostly paths, and
§12 already resolves a relative link against the checkout root. Both clear the
stored mark after the replacement — without that the mark is live at the caret
and the rest of the sentence joins the link (measured: `[here](p) e pronto`
linked *"here e pronto"*).

**A link is the card's ink, underlined.** It was `--crepe-color-primary`, the
theme's `--console-text`. MEASURED across the eighteen ground-and-ink pairs:
8.2:1 on a wash card and **1.94–2.27:1 on every solid one**, where the ink
around it runs 5.4–6.3:1 — the one word asking to be clicked was the least
readable on the line. Following the ink, a link is never harder to read than
its own paragraph on any pair, and the underline is what marks it. Scoped to
the card: the viewer's markdown keeps the theme's link colour, because it is
drawn on the theme's surface.

**The link tooltip wore the card's ink on the plane's chrome.** Upstream leaves
`.link-display` at `color: unset`, so the preview inherited `--note-ink` while
sitting on `--surface` — measured at about 1.3:1 on the default card, a path
that could not be read at all. The floating boxes are chrome, like the toolbar
and the slash menu beside them, and wear the chrome's ink.

**The footer says when the note last landed.** `note.read` now carries the
file's mtime beside its markdown (`note::modified`, a read of its own rather
than a second value out of `read`, whose contract is "what does this note
say"), so the stamp survives a reload — which is exactly when a card has no
memory of a save of its own. A successful write stamps from the client's
clock, because `note.write` replies with no time and a second round trip would
be a read per keystroke-pause. Same day shows the time, any other day shows
the date as well.

**The close toast is the sentence.** `note closed · <path> kept` said it twice
and reassured nobody about a file the `✕` never touches. It is
`note <path> closed`, with the undo unchanged.

## Amendment (2026-09-22, sixth): a note can open veiled, and a veiled card mounts no editor

A note holds what a note holds, and some of it is not for whoever walks past
the screen. The operator asked for an eye.

**`hidden: true` in the front matter, and the card mounts no editor.** Not a
blur and not `display: none` over the text: a veiled card never puts the body
into the document at all, and the markdown it keeps in memory is cut down to
its header. Revealing **re-reads the file**, which is what waking from
dormancy (§14) already does and for the same reason — the file is the note.
This is the guarantee the cheaper answer does not give: with a blur, the text
is one `filter: none` away in the inspector.

**What this is and is not.** It is a guard against the room, and it is not
secrecy. The `.note` container (§3, amended) is deflate under an XOR mask, so
`strings` finds nothing — that is obfuscation, and anyone holding the file and
this source reads it. A note that must be secret from someone with the disk
needs a passphrase and real encryption, which is a decision of its own and not
a line in this one.

**Two controls, because there are two acts.** `⋯ → Hide this note` writes the
mark and is the persistent half; the `👁` in the head shows and re-hides for
**this session only** and writes nothing, so a note looked at once opens veiled
again next time. One button could do both only by marking and never unmarking.
The eye appears only on a marked note — the rule `Use another file…` already
follows — and its glyph names the act it offers, not the state it is in.

**A veiled card refuses the writes that would overwrite it.** The palette and
the rename are refused while veiled, and that is not tidiness: the card is
holding a header where the note used to be, and both of those writers rewrite
the whole document from what the card holds. `withStyle` and `withTitle` carry
the mark along for the mirror reason — a recolour must not quietly un-hide a
note. Dormancy stands down too: a sleeping card replaces the body with its
title, which would overwrite what the veil is saying.

## Amendment (2026-09-22): the lock pins the card, the eye is always there, and a path is a promise

**A note's lock is about PLACEMENT, not about writing.** §8 gave the lock two
jobs — refuse the drag and the resize, and put the editor in read-only — and
the second one is wrong (the operator, seeing it: *"o cadeado no notes não é
impedir de editar é impedir de mover"*). A card is pinned to a spot on the
plane so a fence tidy-up or a stray drag cannot move it; that says nothing
about whether the note may be written. A locked card is typed into, renamed,
recoloured and hidden like any other. What stays is the gesture half: the
resize bands go inert, the head stops offering a drag cursor, and
`makeDraggable`/`startResize` refuse. The editor is never put in read-only by
the lock, and neither the palette, the rename field nor the veil is gated on
it.

**The eye is on every card.** It was present only on a note already marked
`hidden: true`, which made it a control for a state you could only reach
through the `⋯` menu — a door visible only from inside the room. It is drawn
on every card now and carries the act it offers: *hide this note* (the file
write, on an unmarked note), *show this note* (the session reveal), *hide this
note again* (putting it away for this session). Unmarking stays in the `⋯`
menu, beside the other writes to the file.

**A path in the desk record is a promise that a file is there.** The first
save probed for a free name and patched the record with it *before* the write —
so a write that never landed left a card pointing at a file nobody created, and
the next read of that path (a wake from dormancy, a reveal, a detach) turned
the card into §11's missing state. Measured from the operator's screenshot on
2026-09-22: a translucent card in a fence reading
`.ralphy/notes/note-…​.note — not found`, whose text had never reached the
disk. The name is now CLAIMED on the card and recorded only when bytes have
landed; a second flush before that uses the claim rather than probing again and
stepping to `-2`.

And §11's missing state no longer empties the body: a card still holding an
unsaved edit keeps it on screen and says what failed in the footer. The card
stays — that was always the clause — but emptying it is the one irreversible
thing this state can do.

**A missing card that is DIRTY writes its file back** (the operator's decision,
asked and given on 2026-09-22: *"pode regravar o arquivo sumido, aceito"*).
§11 refused every write from a card in this state, reasoning that a note must
not recreate a file behind the operator's back. That reasoning holds for a
card that is CLEAN — all it has is a stale copy of a file somebody deleted —
and it does not hold for one holding text the operator just typed: refusing
there is not caution, it is dropping their writing on the floor. So `dirty` is
the whole test now, and a successful write clears the missing state, because
the file is there again.

## Amendment (2026-09-22): the file's control lives with the file, and a note has a hand

Three changes to §8's chrome, all asked for by the operator against a running
card.

**The `⋯` became a gear, and moved to the footer's left corner.** It sat in
the head, at the end of the row that holds the palette, the index, the eye,
the lock and the close button — so `Delete file…` opened two pixels from the
control that closes the card. But nothing in that menu is about the card:
every entry acts on the *file*, and the footer is already where the file is
named. The gear is the footer's now, sized to the footer's 0.72em type rather
than to the head's 22 px controls, and its menu opens upward from that corner.
The index keeps the head's corner, because its own trigger never moved.

**`Hide this note` is gone from that menu.** The eye is on every card since
the amendment above, and it hides any of them — the entry had become a second
door to one place. The *other* direction stays, because it is not a duplicate
of anything: the eye can mark a note and reveal it for a session, but it can
never unmark, so deleting the entry outright would strand a veiled note with
no way back. It is shown only on a note that is marked, which is the rule
`Use another file…` already follows.

That door had a defect the move exposed. A veiled card holds only its header —
`mountEditor` cuts the body out rather than put it in a document nobody is
looking at — so unmarking straight from what the card held would have written
that bare header over the note and lost the text. Unmarking now reads the file
back first and rides the load out.

**A note is written in a hand and a size** (`font:` and `size:` in the front
matter, beside `color:`/`fill:`/`ink:`). Both are CLOSED SETS — `sans`,
`serif`, `mono`, and five steps from `xs` to `xl` — and not a free
`font-family` string or a pixel count. A free string would put a font the
writer happens to have installed into a file somebody else opens, and the card
would paint in a fallback nobody chose; the sizes are ratios of ADR-0035's
`--reading-size`, so a note keeps its relation to the rest of the workbench
when that scale moves. The names are roles and the stylesheet owns the stacks.

They are fields of the look like any other, which carries the rest of §8's
rules unchanged: the file is what holds them, so a card closed and reopened
comes back in its own hand; a default is omitted from the header, so turning
this on rewrites no note already written; and the tokens are pointed at Crepe's
own three variables, so the choice reaches the headings inside the editor
rather than stopping at the card's padding. The code font is deliberately left
alone — code in a serif note is still code.

## Amendment (2026-09-22): a diagram is drawn on the card, not in a hole cut out of it

§15 gave the mermaid fence the plane's code-block ground (`--log-bg`) and let
mermaid paint it with its own dark theme. On a card the operator had coloured,
that read as a hole — asked about directly, with a screenshot of an ochre note
holding a slab of near-black.

**How mermaid chooses its colours, and why no built-in theme can work here.**
It derives the whole palette from three or four seeds by lightening, darkening
and *inverting* them — `primaryTextColor` is the inverse of `primaryColor`
unless you say otherwise. It never looks at the page it is drawn on. So its
contrast is always against its own assumed canvas, and the only way to make it
agree with a card is to hand it the card's own two colours as the seeds.

The seeds are now read off the card at draw time — `background-color` is
`--note-ground` and `color` is `--note-ink`, both already resolved. The host
paints nothing, so what is behind the drawing is the note itself.

**The outline carries the drawing and the node carries the label**, and that
split is what took three attempts to get right. The borders and the arrows are
the ink barely held back (85 %): they say which box is a box, which diamond is
a decision and what points at what, so on a solid card they must not dissolve
into the ground — the first cut had them at 45 % and the diagram went to mush.
Every label is the ink at full strength.

The node's own fill moves **away from the ink**, not away from the ground.
Shading it against the ground made boxes that read as holes punched in the
note — a quieter version of the complaint the built-in dark theme earned — and
lifting it always made a light card with a light ink paler still. Away from the
ink is the one direction that holds for every combination the palette allows,
because the label is what has to be readable and the outline carries the shape
either way. Over a dark ink the node goes half toward white, which on an ochre
card is the kraft-and-cream of a printed diagram; over a light ink it goes down
instead, and by a fifth, which seats the shape without making it a slab of some
other colour.

Three things this cost, each measured rather than reasoned:

- The palette travels as a per-diagram `%%{init}%%` directive, not as a second
  `initialize()`. That call is global and two cards can render at once — the
  second would repaint the first in its own colours, and the race is invisible
  until two notes are open side by side. `securityLevel` is on mermaid's own
  `secure` list, so `strict` holds whatever a note's bytes say.
- The first draw of every fence runs on a DETACHED node: ProseMirror inserts a
  node view's `dom` after the constructor returns, where `closest` finds no
  card and `getComputedStyle` answers with empty strings. The draw waits a
  frame for the node to be placed, and gives up after a handful — a node view
  can be built and discarded without ever being inserted.
- A card's ground may be a `color-mix()` (every `wash` fill is), which Chrome
  resolves to `color(srgb …)` and not to `rgb(…)`. Reading only the second form
  left every washed card silently falling back to mermaid's stock palette.

And because mermaid writes its colours into the SVG as inline fills, the
cascade cannot reach them: restyling a card has to redraw the diagrams in it,
or they keep the tone the note used to be. Each host remembers the source it
was drawn from, and `restyle` asks the editor to repaint them.
