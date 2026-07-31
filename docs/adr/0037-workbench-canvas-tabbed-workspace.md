# Workbench canvas: a tabbed workspace with a fixed Consoles tab

Status: accepted (promotes a decision hardened in the shell during PRD #185 and
flagged as a candidate ADR by the workbench build guide; extracted from
`crates/ralphy-daemon/assets/ui/`).
Amended by #305: the fixed tab is renamed Agents → Consoles (display name and
identifier `consoles`); the decision itself is unchanged.
Amended by #342: **Arrange** is retired from the strip's console controls in §2
— tiling now lives in each fence's own chrome, per ADR-0051 §7.
Amended by #358: §3 is widened — a closable tab may also be a **daemon view**,
not only an open file.

The daemon workbench (ADR-0032, promoted to the daemon's `/` in #200) lays out
as four columns: **icon rail · sidebar · canvas · Runs panel**. This ADR records
what the **canvas** — the central pane — *is*, so future panes attach to the
established structure instead of reinventing it. It is a reference document, not
a new decision; the shape below is the one already shipped. Vocabulary
(**canvas**, **Consoles tab**, **tab strip**, **workbench session**, **free
console**) lives in [CONTEXT.md](../../CONTEXT.md); the visual language lives in
[ADR-0035](0035-daemon-ui-visual-language.md).

## Decision

### 1. The canvas is a tabbed workspace, not a single view

A **tab strip** (`.tabbar`) runs across the top of the canvas. The pane below it
belongs to whichever tab is active. Tab state and lifecycle live in
`app.js` (`tabs`, `active`, `activate`, `openTab`, `closeTab`); the panes
themselves are owned by the viewer and console modules.

### 2. Tab 0 is the fixed Consoles tab

The first tab is the **Consoles** tab. It is **fixed**: it never closes and cannot
be reordered away from position 0. It hosts the floating agent consoles (the
**workbench sessions** and **free consoles**) over the dotted stage. The console
controls (**New console ▾**, **Arrange**) are pinned to the strip's right edge,
above the workspace, so a floating console can never cover them.

### 3. Files ride in as closable tabs after it

Every opened file becomes a **closable** tab inserted after the Consoles tab. On
open, the pane is chosen by extension (`classify`): markdown renders, binaries
are refused (`open-refused`), everything else opens as source. Closing a file
tab never touches the Consoles tab or its consoles.

### 3b. A closable tab may also be a daemon view (amendment, #358)

The original §3 read as an equivalence: a closable tab **is** an open file, and
the Consoles tab was the single exception. The **Spend** tab (PRD #355) is the
first thing that is neither — it is a **daemon view**: a pane whose content is a
document the daemon serves (`/api/spend`), scoped to the open project, with no
file behind it and nothing on disk to save.

So the invariant is widened, deliberately and once, rather than broken quietly by
each new pane:

- A closable tab is either an **open file** (§3) or a **daemon view**.
- A daemon view's tab id is a **stable literal** (`spend`), not a
  `file:<project>:<path>` key. One tab per view, and re-opening it from the icon
  rail activates the existing one instead of stacking duplicates.
- A daemon view is **never** in the per-client view store (`wb.view.v1`, ADR-0051
  §8). That store restores *open files*; a view's content is derived from live
  daemon state, so restoring it would resurrect a reading of state that has since
  moved — the same reason a `diff:` tab is already excluded.
- Its subject may change while it sits in the background (the Spend tab's subject
  is the open project), so a daemon view **re-reads on activation**. A file tab
  holds bytes; a view holds a question.
- It stays subject to §2: it is inserted after the fixed Consoles tab and can
  never take position 0.

What did **not** change: the Consoles tab is still fixed and still the only
non-closable tab, and an overlay (the Kanban board) is still an overlay. The
choice between "tab" and "overlay" remains the one in Consequences below — this
amendment only says that having chosen *tab*, the content need not be a file.

## Rejected alternatives

- **A single-view canvas that swaps content (agents *or* a file, never both).**
  Rejected: a human running an agent console needs to open a file without
  tearing down the console. The fixed Consoles tab keeps the live sessions present
  while files come and go beside them.
- **Making the Consoles tab an ordinary, closable tab.** Rejected: the consoles
  are the workbench's reason to exist; a stray close (or reorder) that hides
  them is a footgun with no upside. Fixing tab 0 removes the failure mode.
- **(#358) Making the Spend view an overlay, like the Kanban board.** Rejected:
  an overlay is for something you consult and dismiss, over work you are not
  leaving. Cost is read *against* the work — beside the file the money bought —
  and an overlay cannot be left open beside anything. The board earns its overlay
  by being a full-bleed surface with its own drawer; a project total is not.
- **(#358) A second fixed, non-closable tab.** Rejected: §2's fixedness is
  earned by the consoles being the workbench's reason to exist, where a stray
  close is a footgun with no upside. Closing the Spend tab costs one click to
  reopen and loses nothing — making it permanent would spend a scarce strip slot
  on a view the operator only sometimes wants.
- **A separate window/panel for file viewing** (outside the canvas). Rejected:
  it fragments focus and duplicates the tab machinery; one strip owning both
  agents and files is simpler and keeps the floating-console reflow math
  (`clampAll` on `#workspace`) in one place. (The rejection stands; its reason
  is superseded by [ADR-0051](0051-consoles-stage-plane-and-fences.md) §4 —
  `clampAll` is deleted, and the geometry that replaced it is the stage extent,
  still in one place.)

## Consequences

- New canvas content is a **tab**, not a new region — it joins the strip after
  the Consoles tab and obeys `openTab`/`closeTab`.
- A pane that is not a file now has a **contract to inherit** (§3b) instead of a
  precedent to copy: the next daemon view takes a literal id, stays out of the
  view store, and re-reads on activation, without relitigating any of it.
- The Consoles tab's fixedness is load-bearing: code that reorders or closes tabs
  must special-case index 0. This invariant is the decision, not an accident of
  the current `app.js`.
- Overlays that are explicitly *not* tabs (e.g. the Kanban board, which opens as
  an overlay over the canvas) stay overlays; they are a deliberate exception to
  "new canvas content is a tab," recorded here so the distinction is intentional.
