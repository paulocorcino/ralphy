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

### 8. Shared state and per-client state are split

Three kinds of state, three owners:

| state | owner | why |
|---|---|---|
| console output | **shared** | it is a broadcast; watching together is the point |
| who types | **exclusive per session** | the single-writer slot that already exists |
| viewport offset, open file tabs | **per client** | shared, one client's panning would drag the other's view |

The **desk** — windows and fences — stays daemon state, shared, last-write-wins
(ADR-0050 §2). Two people want to see the same arrangement; layout mutations are
rare enough that last-write-wins costs a window in the wrong place at worst.

The **viewport offset and the open file tabs** live in the browser. This is not
the `localStorage` fallback ADR-0050 §3 rejected: that rejection was against a
*second copy of the desk*: authoritative in no mode. This is different state
with a different lifetime, stored once. The cost is stated plainly in the
consequences: the pan does not follow the operator across machines.

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
