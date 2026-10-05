# The consoles stage is a scrollable plane with fences

Status: accepted.

The **Consoles tab** hosts floating console windows over a dotted floor
(ADR-0037), and their placement is daemon state (ADR-0050). Both ADRs assume the
stage *is* the visible box: a rect is absolute pixels inside `#workspace`, and
anything that would not fit is pulled in by `clampAll`, which resizes and
repositions every window from a `ResizeObserver` on the workspace.

That assumption costs the operator their layout. Measured on the shipped build,
1400×900 → 800×600 → 1400×900 with nothing touched by hand:

```
before:  [40,40,600,380]  [700,300,600,380]   workspace 1052×854
shrunk:  [ 0,40,452,380]  [  0,134,452,380]   workspace  452×554
back:    [ 0,40,452,380]  [  0,134,452,380]   workspace 1052×854   ← does not return
canvas overflow: hidden · scrollable: false (all three states)
```

Two windows placed apart were stacked into column 0 with their width cut, and
restoring the browser size did **not** restore them — the deformation is
permanent, and the next interaction persists it to the desk. `clampAll` is not a
bug; it is the only defence against `overflow:hidden` clipping a window into
somewhere unreachable. The wrong part is the premise: that the stage cannot be
larger than the window looking at it.

This ADR replaces that premise with a **plane** the viewport scrolls over, and
adds the unit the operator asked for to organise it — a **fence**. Vocabulary
(**stage**, **viewport**, **fence**, alongside **canvas**, **Consoles tab**,
**desk layout**, **workbench session**) lives in [CONTEXT.md](../../CONTEXT.md).

## Decision

### 1. The stage is a plane; the viewport is a window onto it

`#workspace` becomes the **viewport** (`overflow:auto`) and gains one sized
child, the **stage**, which holds the windows. A desk `rect` is still absolute
pixels — its *reference frame* changes from the viewport to the stage. Migration
is therefore free: with the stage origin at the viewport origin, every existing
desk reopens exactly where it is today.

Resizing the browser changes only `scrollLeft`/`scrollTop`, never a window's
rect. The dotted floor moves from `.canvas` to the stage, or panning would not
read as movement — the background would sit still while everything slides.

### 2. The extent grows on demand, from a pinned origin

The stage is sized to the bounding box of its windows and fences, unioned with
the viewport, plus a margin of breathing room. A fixed giant stage (8000×8000)
is rejected: the scrollbar would measure emptiness and mean nothing.

The origin is pinned at **0,0** and the plane grows right and down only.
Negative coordinates would require re-anchoring the origin and rewriting every
rect on the first drag past the top-left edge.

*(Amended after shipping #340–#343: "a margin of breathing room" was a 200 px
constant, and that is not enough margin to make §7's map work. Scrolling an item
flush to the viewport's top-left corner needs `scrollLeft = item.left`, and the
ceiling is `extent - viewport` — so with a 200 px margin every item except the
furthest one stopped mid-screen, and a second fence could not be brought to the
corner at all. The breathing room is now **`max(200, viewport)` per axis**: the
plane always carries a full viewport of room past its furthest content. This is
not the fixed 8000×8000 rejected above — the extent still derives from the
content, an empty stage is still exactly the viewport, and the scrollbar still
measures a real reachable space, one screen of which is deliberate room to
work.)*

### 3. No zoom

The windows are `xterm.js` terminals with the WebGL renderer. Under
`transform: scale()` the glyph atlas — rasterised at native size — blurs, the
`FitAddon` computes the wrong rows/cols, and selection hit-testing desyncs from
what is drawn. Overview comes from the fence list (§7), not from a scaled stage.

### 4. `clampAll` is deleted; nothing is resized to fit

No window is ever moved or resized on the operator's behalf. This supersedes
ADR-0050 §4, whose smaller-screen story was "`clampAll` already refits a desk
saved on a larger monitor" — on a plane there is nothing to refit, because
nothing is out of reach. It is replaced by an explicit **bring into view**: a
restored window far from the current view must be reachable without blind
scrolling.

### 5. What stays pinned to the frame, not to the plane

- **Maximize** fills the **viewport**, not the stage.
- The `canvas-foot` pills and the empty-stage hint stay in the frame; today they
  are `position:absolute` inside `.canvas` and would scroll away.
- Drag and resize bounds become the stage.

*(Amended 2026-09-26, columns. A maximized console can open other consoles
beside it. Together they are the **columns**: consoles of equal width that fill
the viewport, ordered left to right. The columns are still a maximize, so the
first bullet above holds for all of them: they fill the viewport, not the
stage, over what the operator is looking at.)*

- ***The leftmost column is the maximized console.*** *Only that console has
  `maximized` in the desk record. The other consoles keep their desk rects, and
  a column never changes them: it only paints the console in the viewport, so a
  restore puts each console back where it was. Restore removes one column, and
  the others widen to fill the room. If the leftmost column is removed, the next
  one becomes the maximized console, and that is written to the desk as an
  ordinary maximize. With one column left, it is an ordinary maximize.*
- ***A new column opens directly to the right of the column that asked.*** *So
  opening a column never moves the leftmost one, and only a restore or a swap
  changes which console the desk records as maximized.*
- ***Any column can swap its console, at any time.*** *(Amended 2026-09-27.)
  Each row of the list has a small swap control. It puts that console in the
  column that opened the list. The console that was there goes back to its
  place on the plane, and its session keeps running. A console that is already
  in another column changes places with it, which is also how the columns are
  reordered. At the cap the list still opens: no row can open a new column,
  but every row can swap. A swap of the leftmost column moves the desk's
  maximize to the console that comes in, as a restore of the leftmost does. A
  lone maximized console can swap too.*
- ***The operator decides how many columns, not the app.*** *(Amended
  2026-09-28; this replaces the 80-cell cap below.) Wider than a phone, the
  number of columns has no limit. How narrow a column gets is the operator's
  choice, and the console text size in Settings is how they control it: a
  smaller font puts more characters in the same column. At a viewport of 560 px
  or less (the workbench's phone width) the cap is 1, and the control that
  opens a column does not appear. This still tests the width, not a device
  type. When the viewport narrows to a phone width while columns are open, the
  list is kept and only the leftmost is painted, the same rule the viewer's
  slot follows (ADR-0037 §3c). The others come back when the room does.*

  *Superseded (2026-09-26 text): `cap = floor(viewport width / width of 80
  character cells)`, with the cell width measured on the leftmost terminal.
  Measured on the operator's notebook, this hid the control at the default
  font, and there was no Settings field to change the font: only the key bar's
  A−/A+, which a notebook does not show.*
- ***A column shows less title bar.*** *It keeps the add-a-console control,
  restore, restart and the worktree picker. Lock, fullscreen and close are hidden:
  nothing moves inside the columns, so a lock means nothing there, and close
  would put the end of a working agent one click away in the view where the
  operator is working. Drag and resize are already gone, as for any maximized
  console. Restart stays because a console that is not running can be opened
  in a column, and it must be possible to start it there. The same reason
  holds for a single maximized console: it hides lock and close, and Restore
  takes it out first. A console in fullscreen keeps only the exit and restart:
  maximize changes nothing there, and the add-a-console control, lock and
  close are hidden as well.*
- ***Alt+Shift+←/→ moves the focus between columns while columns are open***,
  *and wraps at the ends, as the fence walk does (§7). Under a maximize the
  fence walk pans a plane the operator cannot see, so the keys lose nothing
  useful.*
- ***Changes from outside.*** *A session that ends keeps its column: the
  console becomes a placeholder and is restarted from the column. A window
  closed by another client leaves its column. Detaching a fence that holds a
  column's console removes that column first, because a terminal is in one
  place at a time (§8). A maximize changed on another device does not reach
  this client's columns until its next restore, where the desk wins (§8).*

*(Amended 2026-09-29, rows. A column can hold more than one console, one above
the other. Each console in a column is a **row** of that column. A column with
one console has one row, so every rule above still holds when no column has a
second row.)*

- ***One control, two directions.*** *The title bar control is now "Add a
  console". It opens the same list as before. At the top of the list, a choice
  of two directions, "Right" and "Down", decides what a click on a console
  in the list does. Right opens a new column directly to the right of the
  caller's column, with that console as its only row. Down opens a new row
  directly below the caller, in the caller's column. The swap control is not affected by the choice. The last
  direction used is kept per client, with the column list (§8). With nothing
  stored it is Right, so an operator who never picks Down sees the same list as
  before.*
- ***Equal sizes, two levels.*** *Columns have equal widths, as before. The rows
  of one column have equal heights, and each column divides its own height. A
  row never holds columns: the shape is a list of columns, each a list of rows,
  and never a deeper tree.*
- ***The first console in reading order is the maximized console.*** *Reading
  order is column by column, left to right, and top to bottom inside a column.
  The first console is the top row of the leftmost column, and it replaces "the
  leftmost column" in every rule above. Restore removes one row, and the other
  rows of that column grow to fill its height. A column with no row left is
  removed, and the other columns widen. If the first console is removed, the
  next one in reading order becomes the maximized console, written to the desk
  as an ordinary maximize.*
- ***A swap works on rows.*** *The swap control puts a console in the row that
  opened the list. A console that is already in another row, in any column,
  changes places with it.*
- ***No row limit.*** *As with columns (2026-09-28), the operator decides how
  short a row gets, with the console text size. At a phone width only the first
  console is painted, and the control does not appear.*
- ***Alt+Shift+↑/↓ moves the focus between the rows of a column***, *and wraps at
  the ends, as Alt+Shift+←/→ does between columns. Alt+Shift+←/→ goes to the
  row at the same position in the next column, or to its last row when that
  column has fewer rows.*
- ***A row has the same title bar as a column***, *and the same rules for
  changes from outside, with "row" in place of "column": a session that ends
  keeps its row, and a closed or detached console leaves it.*

*(Amended 2026-09-30, a fence opens as columns. A button in the fence title
bar opens every console of the fence as columns in one click, instead of one
console at a time from the list.)*

- ***The grid follows the stage.*** *The consoles are the fence's members
  (§6), placeholders included, each read at its stage rect. They are sorted by
  left edge. A console joins a column when its horizontal overlap with the
  column's first console is more than half the width of the narrower of the
  two; otherwise it starts a new column. Only the first console of a column is
  compared, so a staircase of windows does not chain into one tall column.
  Columns are ordered by the left edge of their first console, and rows by top
  edge. The grid is similar to the stage, not equal to it: sizes stay equal
  (see above).*
- ***It replaces the columns that are open.*** *Nothing is lost: a console that
  leaves the columns is only painted at its stage rect again, and the old
  maximized console stops being maximized.*
- ***There is no "restore all".*** *Each console is restored with its own
  restore, one row at a time. A column never changes a stage rect, so every
  console goes back to where it was.*
- ***When the button acts.*** *With one console, it is an ordinary maximize.
  With no console, it is disabled. It is hidden on a detached fence, whose
  consoles are in the popup, and at a phone width, where only one console is
  painted. A lock does not stop it: nothing moves on the stage.*

### 6. A fence is a named, anchored rect; membership is derived

A **fence** is a named rectangle anchored on the stage — `id`, `name`, `rect`,
`ts` — drawn on a floor tier **below every window**, never taking a window's
drag.

- **Fences never overlap.** Enforced on create, move and resize.
- **Membership is derived from the window's centre point**, not stored. A window
  is in the fence whose rect contains its centre. There is no `fenceId` on the
  desk record: position is already the persisted truth, and storing both creates
  an invariant to reconcile ("the record says fence A, the rect sits inside B —
  which wins?"). Drag in, it is in; drag out, it is out. Non-overlap is what
  makes this containment total.
- **Moving a fence moves its members.** That is what anchoring buys, and what
  makes a fence a group rather than a drawn box.

*(Amended for fence detach (#344): membership is derived **on the plane**, and a
detached fence's popup does not participate in that derivation — it holds the
set of windows it was handed at the instant of detach. A window another client
drags out of the fence meanwhile stays in the popup. This does not reopen the
`fenceId` this section rejected: the popup owns no rect that outlives it (§8),
so there is nothing to reconcile against, and the snapshot is the only reading
consistent with the consoles returning to the positions they left. Derivation
resumes untouched the moment they come home.)*

*(Amended 2026-09-20, lock: the tuple is `id`, `name`, `rect`, `ts`,
**`locked`**. A locked fence refuses move, resize and tile, and the consoles it
holds refuse a drag and a resize for as long as it holds them — decided from
the same centre-point fold at gesture time, so membership stays derived and
nothing here reopens `fenceId`. A console locked on its own record inside an
*unlocked* fence is still carried by the fence's move (the fence is the group;
lock the fence to freeze the group) and is skipped by tile, as a maximized one
is. See ADR-0050's lock amendment for the record shape.)*

### 7. Arrange is per fence, and the fence list is the navigation

The global **Arrange** button is retired: on a plane, "tile everything" has no
meaning. Arrange moves into each fence's own chrome (name · window count ·
arrange) and tiles that fence's members into that fence's rect — the existing
`arrange()` generalised to take a rect and a member list.

The **fence list** (name · repos contained · window count) is how the operator
navigates: clicking a name slides the viewport to that fence — a map with
anchors, without zoom. It takes the toolbar slot Arrange vacates; promoting it
to a sidebar view waits for measured use. No minimap (§3 removed the need) and
no roll-up (collapsing a fence is comfort, not foundation).

*(Amended after shipping #343, on the operator's own reading of it:)*

- **The list and the create button are ONE toolbar control.** `Fence` opens a
  menu whose first row draws a new fence and whose remaining rows are the map.
  Two buttons for one noun made the operator learn which of them held which
  verb; this is the shape `New console` already uses.
- **The jump anchors the fence's top-left corner**, one inset in from the
  viewport's, rather than centring it. A fence is a region worked *inside*, not
  a point of interest looked *at* — centring wasted the screen above and left of
  it. The centring fold stays for the Go-to picker, where the target is a single
  window and therefore is a point of interest. §2's amended headroom is what
  makes the corner reachable at all.
- **The jump animates** (~260 ms, cancelled by any pan or wheel, skipped under
  `prefers-reduced-motion`), so the operator sees which way the plane moved. A
  hard cut across a large plane reads as a redraw, not as travel.
- **Under a maximize the jump does not move the view.** *(Amended
  2026-09-27.)* The fence still takes the focus, so the next console is still
  born inside it. The maximized console (or the columns) covers the plane, so a
  slide shows nothing, and the console is moved again on every frame of the
  slide to stay in place.
- **Alt+Shift+←/→ walks the fences** in the plane's reading order — top band
  first, left to right within it — reusing the accelerator idiom the console
  digits already established. Creation order was rejected: on a plane it
  teleports across the stage.

### 7a. Every edge of a fence is a handle

A fence resizes from all four edges and all four corners, not just the SE grip.
The west and north edges move `left`/`top` with the opposite edge anchored — so
this is still a resize and never §6's move, and it carries no members: windows
the edge sweeps past simply stop being contained, which is what deriving
membership buys. The pure `resizeRect` the console windows already use answers
all eight directions unchanged.

Arrange and close leave the head band for the fence's **top-right corner**.
Trailing the head made their position a function of the fence's name length and
repo list, so the same control sat somewhere different on every fence.

A fence is **free-form**, not bound to a project: a fence mixing repos (a
cross-repo task) and two fences for one repo ("planning", "executing") are both
legitimate. The chrome *shows* the repos it contains. A console opened while a
fence is focused is born inside it.

*(Amended for fence detach (#344):)*

- **`detach` joins arrange and close** in that same top-right corner, for the
  reason this section already gives: a fence verb whose position is a function
  of the fence's name length sits somewhere different on every fence. It opens
  the popup of §8.
- **A detached fence is otherwise an ordinary fence.** It appears in the fence
  list, `Alt+Shift+←/→` walks to it in reading order, it moves and resizes on
  the plane, and a console born while it is focused is still born in it *on the
  plane* — the popup has no launcher (§8), so the birth rule above gains no
  exception. Exactly two things change: its member windows are not rendered,
  and **arrange becomes a no-op** rather than a visible non-event tiling an
  empty rect.
- **The emptied rect carries a detach glyph, and the glyph is the control.** An
  empty fence caused by a detach must never read as an empty fence that simply
  holds no consoles — the fence list would report the same count for both.

  *(Amended 2026-07-28. The glyph carried two intents — re-attach, or focus the
  popup when one was already open behind other windows — told apart by state the
  operator cannot see. It reads as a re-attach button, so it **is** one: one
  click closes the window holding the consoles and puts them back in the fence,
  whether or not this document opened that window, and whether or not the
  registry still agrees it exists. Raising a buried popup did not disappear; it
  moved to the head's `detach` button, where a fence already detached answers
  `focus` — and that button is the one an operator presses to ask "show me this
  fence's window". A verb that changes meaning with hidden state is not one
  control for two intents, it is two controls wearing one glyph. The glyph is
  also hidden **in CSS**, not only by the `hidden` attribute: an author `display`
  rule beats the UA's `[hidden]`, and for a while it offered the way home on
  every fence, including the ones that had never left.)*

### 8. Shared state and per-client state are split

Three kinds of state, three owners:

| state | owner | why |
|---|---|---|
| console output | **shared** | it is a broadcast; watching together is the point |
| who types | **exclusive per session** | the single-writer slot that already exists |
| viewport offset, open file tabs and the slot beside the active one (ADR-0037 §3c) | **per client** | shared, one client's panning would drag the other's view |
| which fences are detached | **per client, per tab** | shared, one operator's second monitor would empty a fence on the other's screen |
| which consoles and fences are locked | **shared** | a lock protects the layout itself, which every device shows; per client, the device that slips (the tablet) is the one that would forget it |
| which consoles are open as columns and rows (§5), and their order | **per client** | shared, one device's columns would open on a screen that has no room for them |

The **desk** — windows and fences — stays daemon state, shared, last-write-wins
(ADR-0050 §2). Two people want to see the same arrangement; layout mutations are
rare enough that last-write-wins costs a window in the wrong place at worst.

The **viewport offset and the open file tabs** live in the browser. This is not
the `localStorage` fallback ADR-0050 §3 rejected: that rejection was against a
*second copy of the desk*: authoritative in no mode. This is different state
with a different lifetime, stored once. The cost is stated plainly in the
consequences: the pan does not follow the operator across machines.

*(Amended 2026-09-26, columns. The column list joins the viewport offset and
the slot in `wb.view.v1`. That is the lifetime of a browser profile, not the
per-tab lifetime of a detach below: the list survives a reload and a new tab,
and two tabs of one browser share it, as they already share the slot. It holds
window ids and nothing else. On restore it is checked against the desk: ids
that no longer exist are dropped, and if the desk no longer records the list's
first console as maximized, the list is ignored. The desk wins because it is
the state every device agrees on.)*

*(Amended 2026-09-29, rows. The stored list becomes a list of columns, each a
list of window ids, plus the last direction used (Right or Down). A stored flat
list, written before rows, reads as one row per column, so no stored layout is
lost. The check on restore is the same: unknown ids are dropped, an empty column
is dropped, and the list is ignored unless its first console in reading order is
the one the desk records as maximized.)*

*(Amended for fence detach (#344). The plane answers "where is my work"; it does
not answer "I have a second monitor". §3 forbids the zoom that would let two
regions of the plane fit on one screen, and a second `#workspace` in the same
document is still one browser window on one piece of glass. So a fence
**detaches**: a real `window.open` popup — an OS window the operator drags to
the other monitor — holding a small stage with that fence's consoles, while the
origin fence stays on the plane, in place, empty, glyphed (§7a). Closing the
popup, or clicking the glyph, brings them home — and the glyph does both: it
closes that window itself.)*

- **Detach is per client and per TAB**, so it lives in the acting tab's
  session-scoped storage rather than beside the `wb.view.v1` key above. This is
  a third lifetime, not a third copy: the viewport offset should survive into a
  new tab, and a detach must not — a popup is bound to the document that opened
  it, and a second tab inheriting "that fence is elsewhere" would empty a fence
  whose consoles it has no window for. A second browser opening the same URL
  therefore sees an ordinary fence with the consoles inside it, and keeps
  watching output detached elsewhere, because output is a broadcast (§9).
- **The popup is a child of the tab.** It survives that tab's reload — an F5 is
  a reflex, not a decision, and must not cost the operator their second screen —
  because the link is re-established over a same-origin broadcast channel rather
  than an in-memory window handle, which is precisely what a reload destroys. A
  heartbeat covers the case where no unload fires, and a peer silent past it
  closes the popup, after saying so. Without that rule a closed origin tab
  leaves an orphan driving sessions while a fresh tab renders the same consoles
  inside the fence, and "the consoles live only in the popup" stops being true.
  *(Amended 2026-09-26, #476: each popup has an identity, a `pid` the opener
  gives it at detach and sends in the handover. Every message a popup sends
  carries it, and the opener ignores a `popup-*` message from another popup
  of the same fence; `popup-ping` is the one exception, because answering a
  probe does no harm. Of the opener's messages, only `origin-close` and
  `origin-focus` carry a `pid`; `origin-here`, `origin-beat` and
  `origin-ping` are broadcasts to every popup of the tab. A popup that is
  unloading answers nothing, so it cannot be the popup a reloaded opener
  adopts. Without it, a popup that is closing after a re-attach still
  says `popup-gone` for its fence, and when the fence has been detached again
  in the meantime, that message re-attached the NEW popup and closed it. Its
  `popup-members` could also drop the new popup's consoles from the desk. An
  opener that reloaded has no `pid` for the fence until the popup's first
  `popup-here`, and it adopts the `pid` from that answer. An order with no
  `pid` comes from such an opener, and the popup still obeys it. The `pid` is
  not written to the tab's registry.)*
- **The popup never writes the desk.** Its internal layout is throwaway: the
  operator lays it out for that window's shape, and the consoles return to the
  positions they left. The desk-write path is therefore reached through an
  **injected sink** — the shell passes the daemon-backed one, the popup a null
  one — and not a `detached` flag branched at each persistence call site. The
  failure mode worth designing against is one write leaking through a missed
  branch months later; an injected sink makes the popup *incapable* of writing.
  *(Amended 2026-09-26, #475: the popup sends REPORTS about a note's name.
  When a card in the popup chooses a name for a note that has no file, the
  popup tells the opener that `claim`, and the opener keeps it on the member.
  When that first write lands, the popup tells the opener the path, and the
  opener writes it to the desk after it checks that the note is a card this
  popup holds, that its record has no path yet, and that the path is relative
  and ends in `.note`. The popup still has the null sink and writes nothing
  itself. Each report is sent twice, over `postMessage` and over the lifecycle
  channel, so it arrives before the popup's own re-attach message and also
  after the opener reloads. The members that `popup-here` hands back carry the
  name too, so a report the opener missed during its reload is recorded when it
  adopts the popup again. The snapshot of §6 may carry `draft` and `claim` for
  such a note. They exist only in the snapshot and in the popup's copy of it,
  never in the desk or in the tab's detach registry, which holds ids only.)*
- **The popup opens no consoles**, so its contents are exactly §6's snapshot and
  re-attach stays a well-defined inverse.
  *(Amended 2026-10-05, tile in the popup. The popup has the fence head's
  Tile, as a button in its top-right corner. It tiles the consoles into the
  part of the stage the window shows, below the button, with the same rules
  as the fence: a maximized, column or locked console is not tiled. The
  layout stays throwaway, because the popup's sink writes nothing. Notes do
  not move (ADR-0064 §8), but the tile raises them above the consoles: the
  popup has no Note menu to bring a covered card back. A phone does not show
  the button: it paints one console, so it has nothing to tile.)*
- **At most four popups, and one per fence** — detaching an already-detached
  fence focuses its popup. This cap is a **client** constant and deliberately
  does not sit beside §10's daemon-enforced ones: a detach *moves* consoles
  rather than creating them, so the live-terminal count — the real resource,
  each holding a WebGL context — is unchanged and already bounded by the desk
  cap. Four is therefore an ergonomic ceiling, not a technical one: more than
  any real monitor count, while bounding how many documents each load the
  vendored bundle.

### 9. Many clients, one writer per session — and the client never steals

The console flapping between two browser windows is not caused by having two
clients. Measured: every client reconnect carries `takeover=1`, and an eviction
arrives at the browser as `code 1005, wasClean=false` — the daemon's Close frame
is sent but the socket is dropped before the closing handshake completes, so the
"deliberate end" signal the client checks for is never delivered. Each side
therefore reads eviction as a flaky link and reclaims the session, forever, at
roughly 1.1 s per flip.

So:

- **A reconnect never carries `takeover`.** It reattaches without claiming the
  baton, or not at all — so whoever is driving keeps driving, and the daemon's
  `409` makes theft impossible rather than merely unattempted.
  *(Amended while implementing #334: this clause first read "it reattaches as a
  reader". Reattaching read-only on the FIRST retry breaks the two cases the
  acceptance criteria require — a flaky link recovering to a working keyboard,
  and an F5 racing the old bridge's teardown — because the reconnecting client
  is the rightful driver and the slot it wants is its own, moments-ago one. The
  shipped rule keeps the property that matters, `takeover` never on an automatic
  path: a client reconnects as a would-be writer a bounded number of times and
  then settles for watching.)*
- **A busy session is a visible state, not a prompt to steal**: the window shows
  that the session is driven elsewhere, with an explicit *take over* the operator
  clicks. Handing over the keyboard is a deliberate, visible act.
- **The daemon states why it closed** — an explicit eviction reason on the wire.
  The client must not depend on `wasClean`, which the measurement shows it does
  not receive.

*(Amended for fence detach (#344). Detaching a fence moves its consoles to
another window, so the writer slot moves with them — and read carelessly that is
the automatic reclaim this section just deleted. It is not, and the distinction
is worth stating in the same words the rule is written in: what §9 forbids is a
**client claiming on its own, on a machine-driven path** — a reconnect, a
retry, a reopen. A detach is neither automatic nor machine-driven, it is one
operator in one deliberate click, and the slot changing hands is the one the
acting tab already held. Handing over what is yours is not theft.*

*It needs no new mechanism either, which is the tell that the model was already
right. Detaching tears the origin windows down; that closes their sockets;
that releases the slot. The popup then attaches by the ordinary path, with no
`takeover` anywhere — so the daemon's `409` still makes theft impossible rather
than merely unattempted, and the two cases that were never the acting tab's to
give behave exactly as this section already specifies: a session another client
is driving arrives **busy**, with the explicit **take over** the operator
clicks, and a slot lost to a race in the instant between release and attach
falls back to the same visible state. Do not build a "release" verb for this.)*

*(Amended for dormant consoles. A console that has been off the viewport long
enough disposes its terminal and closes its socket, which releases the slot; it
rebuilds and reattaches when it returns. This is the detach amendment's shape
with the operator's click replaced by the operator's attention, and it needs no
new mechanism for the same reason: the wake is the **ordinary attach**, never
`takeover`, so a session another client claimed while this one slept arrives
busy and lands in the visible **take over** state this section already
specifies. The one thing worth saying out loud is that a machine-driven path is
now what RELEASES the slot — which §9 has never forbidden, and could not: a
closed socket is what every reconnect, every tab close and every detach already
does. What §9 forbids is CLAIMING, and nothing here claims.*

*Dormancy is runtime state of one client: never persisted, never written to the
desk record (ADR-0050), never told to the daemon. The daemon's view of who holds
the baton is exactly what it was.)*

*(Amended 2026-10-03, covered consoles. Columns, a maximize and the physical
screen fill the viewport. The consoles under them are still inside it, and the
`IntersectionObserver` reports geometry, not paint, so it called them visible
and they never slept. With any number of columns (§5), the live terminals
passed Chrome's limit of about 16 WebGL contexts, and the canvas of a dropped
context turned white with a sad face. A console that does not itself fill the
viewport, while another console does, now counts as off the viewport: it sleeps
after the same grace period, and it wakes when the cover goes. The rule is the
same pure fold with one more input.)*

*(Amended 2026-10-05, restored consoles. A window that reattaches to a known
session (a desk restore, a fence that comes home) starts asleep, and the first
report of the `IntersectionObserver` wakes it when it is visible. Before, it
attached at once, replayed the text of its session, and slept after the grace
period when nobody could see it: measured on three devices, 42% of the console
bytes went to those replays. The wake is the same ordinary attach as above. A
launch still attaches at once, because it has no session to wake to. Without
an `IntersectionObserver` a window attaches at once, because nothing would
wake it.)*

*(Amended 2026-09-22, the half-open writer. Measured behind a tunnel
(TunnelDeck for dev tunnels): after a phone switched from wifi to 4G, the tunnel
agent kept its legs to the daemon ESTABLISHED for minutes although the browser
behind them was gone. The daemon's 20 s ping went out and was accepted, so the
dead leg kept the writer slot. The tab's own reattach got `409` on every retry.
A window that had held the session gave up after `MAX_FAILED_REOPENS` and printed
`[session closed]`. A reloaded page parked as a watcher and offered **take over**
as if another device were driving. Two rules close it, and neither lets a client
claim a slot it did not hold.*

- ***A writer that answers no ping is gone.*** *The bridge counts what the
  client SENDS, pongs included, never whether its own send succeeded. After two
  ping periods plus slack with nothing heard (45 s at the 20 s ping), it detaches
  silently, like a dropped link. The session lives on, and the slot is free for
  whoever reattaches. A browser pongs from its network stack, even in a
  background tab, so only a client that is really gone goes quiet.*
- ***A tab may reclaim its own slot.*** *Every claim of the slot (launch,
  reattach, takeover) names the tab's **holder**, a random id kept in
  `sessionStorage` so it survives a reload and differs per tab. A reattach
  naming the holder that claimed the slot evicts the incumbent without
  `takeover`. That incumbent is this tab's own earlier socket, and the
  detach amendment's words already cover it: handing over what is yours is not
  theft. A different holder, no holder, or a slot claimed with none still gets
  `409`. The holder is not a credential: any authenticated client could send
  `takeover=1` already, so naming one grants nothing new. It is not the "seat"
  rejected below either, because the daemon keeps it only while the slot is held
  and never persists it. A duplicated tab copies `sessionStorage` and so shares
  the holder. Its first attach reclaims the slot, and the original tab parks
  visibly, the same outcome as an explicit takeover.)*

**Pairing therefore needs no new feature.** A client that has not claimed the
writer slot *is* a spectator: the broadcast channel already serves any number of
readers, and the writer slot is the driver's baton. Two people on one daemon get
"both watch, one drives" out of the model ADR-0032 already has.

### 10. The desk grows a field; the route's body grows a shape

`DeskStore` is already an object with a `windows` field, so `fences` is an
**additive** field and an existing `desk.toml` keeps loading. The wire body
changes: `PUT /api/desk` takes a bare array of records today and becomes
`{ windows, fences }`. A contained break — the shell and the daemon ship in one
binary. Fences get their own daemon-enforced cap and the same `rect_is_sane`
rejection as windows.

*(Amended for fence detach (#344): the detach adds **nothing here**. No wire
shape changes, `DeskStore` gains no field, no cap joins the two above, and the
daemon never learns that a fence is detached — it is presentation state owned by
one tab (§8). This is the load-bearing property, not an implementation note: it
is what makes "another browser sees an ordinary fence" true by construction
rather than by a rule someone has to remember to enforce.)*

*(Amended 2026-09-20, lock: this one DOES add a field — `locked` on the window
record and on the fence — because a lock is desk state (§8, lock row), the
opposite call from detach for the opposite reason. Additive and unserialised
when off; ADR-0050's lock amendment has the shape and the wire rule.)*

*(Amended 2026-09-26, columns: nothing here. No wire shape changes, no desk
field, no cap. The daemon never learns that a console is in a column; it sees
one ordinary maximize, as it does today. The same holds for rows, 2026-09-29,
and for a fence opened as columns, 2026-09-30.)*

## Rejected alternatives

- **An exclusive-client claim ("posse") on the presence socket** — one live
  workbench, a new one evicting the last. Rejected: it was mechanism to hide §9's
  auto-takeover, and it forbids pairing. Deleting the automatic steal fixes the
  defect with strictly less machinery.
- **"One browser per daemon", enforced at login.** Rejected as unenforceable:
  the daemon sees connections, not browsers, and two tabs of one browser carry
  the same cookie — the rule would stand while the flapping continued.
- **Refusing a second client outright.** Rejected: a forgotten or sleeping tab
  holding the claim would lock the operator out of their own daemon.
- **An infinite-canvas library** (panzoom, react-flow and kin). Rejected: they
  bring the zoom §3 rules out and a second coordinate system to fight. The
  mechanism here is `overflow:auto` plus a sized child.
- **The vendored Excalidraw canvas (ADR-0048) as the stage.** Rejected: it
  renders to a pixel canvas, and the windows are DOM elements with terminals
  inside them.
- **Explicit `fenceId` membership.** Rejected: a second source of truth for
  something position already answers (§6).
- **Overlapping fences with a z-order tie-break.** Rejected: ambiguity bought
  nothing.
- **A fence auto-created per project.** Rejected: it would fight the two
  legitimate shapes §7 names.
- **Roll-up (collapsing a fence).** Deferred, not refused — comfort, and
  surface the first slices do not need.
- **The viewport offset in the daemon, keyed by a "seat".** Rejected for now: an
  opaque per-browser seat id is a user account by another name. It is the
  graduation path if pairing sticks.
- **Proportional rects or a desk keyed by viewport size** (already rejected in
  ADR-0050). Still rejected, and now moot: the plane removes the problem they
  were solving.

*(Added for fence detach (#344):)*

- **An in-page overlay instead of a real popup window.** Rejected: it never
  reaches the second monitor, and §3 already rules out the zoom that would
  otherwise let two regions of the plane fit on one screen.
- **A `detached` flag branched at each desk-write call site**, instead of §8's
  injected sink. Rejected: it makes the popup's read-only-ness a matter of
  remembering, and the leak it invites is silent.
- **Storing the detached set in the daemon.** Rejected: it is presentation
  state, and sharing it would empty a fence on a colleague's screen because
  someone else has two monitors.
- **Sharing a detach across two tabs of one browser.** Rejected with the same
  argument at smaller scale: the second tab has no window for those consoles.
- **Opening new consoles inside the popup.** Rejected: it would make the popup's
  contents diverge from §6's snapshot, and re-attach would stop being a
  well-defined inverse.
- **A popup that outlives its opener tab.** Rejected: an orphan drives sessions
  that a fresh tab simultaneously renders inside the fence.

*(Added for columns, 2026-09-26:)*

- **Columns in the desk.** Rejected: a phone would restore columns it has no
  room for.
- **Every column written as `maximized`.** Rejected: every other device would
  restore several full-viewport consoles on top of each other.
- **A fixed number of columns, or a minimum width in pixels.** Rejected: a fixed
  number is wrong on a wide monitor and on a tablet, and a pixel width ignores
  the font size, which each browser profile sets.
- **A minimum column width the operator sets in Settings** (2026-09-28).
  Rejected: the text size already decides how many characters a column holds,
  and a second setting for the same result is one more thing to explain.
- **Removing the columns that no longer fit when the viewport narrows.**
  Rejected: a tablet turned twice would lose them.
- **Nested splits (rows and columns), a divider the operator drags, and
  reordering by drag.** Not in the first version (a swap between two columns
  reorders them without a drag). Equal columns are the
  smallest shape that gives "several consoles I am working in"; the others wait
  for measured use. *(2026-09-29: rows inside a column are now decided, §5. The
  divider and the drag still wait.)*

*(Added for rows, 2026-09-29:)*

- **A free tree of splits, where any row can split into columns again.**
  Rejected: two levels cover the layouts asked for, and a tree needs a divider
  model, a deeper stored shape and a focus walk with no clear order.
- **One direction for the whole layout** (all columns or all rows). Rejected: a
  column cannot then hold two consoles next to a tall one, which is the layout
  that asked for rows.
- **A second title bar button for Down.** Rejected: the title bar of a column
  is already narrow, and both buttons would open the same list.

*(Added for a fence opened as columns, 2026-09-30:)*

- **A "restore all" button.** Rejected by the operator: the restore of each row
  already exists, and each console goes back to its own place.
- **Columns and rows that copy the stage sizes.** Rejected: columns have equal
  sizes (§5), and a divider model is still not decided.
- **Adding the fence's consoles to the columns that are open.** Rejected: the
  result would mix two layouts, and the old columns lose nothing when they are
  replaced.

## Consequences

- **The resize deformation disappears rather than being fixed.** Restoring a
  desk saved on a bigger screen scrolls instead of squeezing.
- **The takeover loop dies by removal**, not by a new ownership layer.
- **Pairing works on day one**: both watch, one drives, the baton changes hands
  with a click.
- **A paired peer has a shell on the host, in every registered repo.** The gate
  is the tunnel plus `require-login` (ADR-0032 §4) — pairing is the scenario
  where enabling login stops being optional.
- **Solo across machines**: windows and fences follow, the pan does not (§8).
- **Two people dragging the same window** resolve last-write-wins, which can
  surprise. If it bites, the answer is a desk per seat — measured first.
- **Arrange changes meaning and place**: muscle memory for the toolbar button
  breaks, deliberately.
- Delivery splits in two: the plane is self-contained and closes the deformation
  on its own; fences land on top of it. Merging them would make the defect wait
  for the new concept.
- CONTEXT.md gains **stage**, **viewport** and **fence** — and its **Desk
  layout** entry, which still says "client-side and per-browser-profile; there is
  no machine-wide store", is corrected: ADR-0050 already moved the desk into the
  daemon.

*(Added for fence detach (#344):)*

- **The second monitor is answered without touching the daemon.** The whole
  feature is per-client presentation over machinery that already exists.
- **A detach does not follow the operator anywhere** — not across machines, not
  into a second tab, not past closing the one that acted. This is the stated
  cost of §8's third lifetime, and it is the same trade the viewport offset
  already makes, one notch shorter.
- **The desk-write path gains an indirection** it did not have. Justified by a
  second caller that exists (the popup's null sink), not "for flexibility".
- **The fence chrome gains a third control** — arrange, close, and now detach,
  over a head band that also carries name, repos and count. If it stops fitting
  at small fence widths that is a
  chrome-density question for the visual language (ADR-0035), not a reason to
  move a fence verb somewhere a fence verb has never lived.
- **The popup carries a shell on the host, like any workbench window.** It is
  same-origin and behind the same gate; ADR-0032 §4's `require-login` posture is
  unchanged, and a popup with no valid same-origin opener renders nothing at all
  rather than something a composed link chose.
- CONTEXT.md gains **detached fence**.
- **The fence chrome gains a fourth control** (lock, 2026-09-20), and the head
  band's reserve widens with it. Same density question as the third, same
  answer.

*(Added for columns, 2026-09-26:)*

- **A narrow column narrows the terminal for every client.** Each attached
  client sends `resize`, and the daemon applies the last one
  (`routes/ws_session/bridge.rs`). This is already true for any window resize
  or maximize; columns make it happen more often. A tablet watching a console
  that is a narrow column on the desktop sees the agent's screen drawn at that
  width. Deciding which client sets the size (for example, the one with the
  focus) is a separate decision, taken if this causes trouble in use.
- **Columns follow neither the machine nor the browser.** Like the pan (§8),
  they stay in one browser profile. The desk still records one maximized
  console, so another device opens that console maximized and nothing else.
- **The daemon is not touched.** The whole feature is per-client presentation
  over the maximize that already exists.
- CONTEXT.md gains **Columns**.

*(Added for the 2026-09-28 amendment, no column limit:)*

- **A column can be too narrow to use.** Many columns on a small screen send a
  terminal only a few characters wide, and an agent CLI draws its screen badly
  at that width. This is the operator's choice, the same as a small floating
  window. A column also drops the floating window's minimum width of 240 px,
  or narrow columns would overlap.
- **Settings gains "Console text size"** (10–28 px, default 15, in this browser
  only). It is the same stored value the key bar's A−/A+ change. A touch screen
  no longer starts one pixel smaller: that default existed only so two 80-cell
  columns fit a landscape iPad.

*(Added for rows, 2026-09-29:)*

- **A short row makes the terminal shorter for every client**, for the same
  reason a narrow column makes it narrower: the daemon applies the last
  `resize`.
- **The stored view changes shape inside `wb.view.v1`.** The reader accepts the
  old flat list, so the key keeps its name.
- **The daemon is not touched**, as for columns.
- CONTEXT.md's **Columns** entry gains rows.

## Amendment (2026-09-30): rules recorded from the glossary

These rules were decided earlier and were recorded only in CONTEXT.md. On 2026-09-30 the glossary was cut back to definitions, so the rules move here without change. Nothing new is decided.

- **The floor pans (§1).** The floor is the **pan** surface: dragging it moves the view and never a rect, and dragging a window against the viewport edge auto-pans.
- **The first landing (§8).** With nothing stored, the view lands on the bounding box of the restored windows; a stored offset that would show no window at all degrades to that same landing, so a smaller screen still lands on work.
- **A watcher is refused by the client too (§9).** A watcher's keystrokes are refused by the client as well: the browser gates its own input and names what it is watching in the window, a visible state rather than a `confirm()` prompt (issue #335), with the daemon's drop kept as defence in depth.
- **The key bar is an input surface (§9).** It is not a menu: every button sends bytes down the same path a keystroke takes, so a **watching** window refuses a tap exactly as it refuses a keystroke.
- **A free console can start with a command.** A free console can start with a command line typed in the Consoles menu: the shell runs it, the session ends with it, and the command is the session's label, so a restart runs it again. Nothing stores the command as a default (decided 2026-09-27, `f0903843`).
