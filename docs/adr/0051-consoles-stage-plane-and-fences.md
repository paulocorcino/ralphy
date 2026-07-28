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

*(Amended for fence detach (#344): membership is derived **on the plane**, and a
detached fence's popup does not participate in that derivation — it holds the
set of windows it was handed at the instant of detach. A window another client
drags out of the fence meanwhile stays in the popup. This does not reopen the
`fenceId` this section rejected: the popup owns no rect that outlives it (§8),
so there is nothing to reconcile against, and the snapshot is the only reading
consistent with the consoles returning to the positions they left. Derivation
resumes untouched the moment they come home.)*

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
| viewport offset, open file tabs | **per client** | shared, one client's panning would drag the other's view |
| which fences are detached | **per client, per tab** | shared, one operator's second monitor would empty a fence on the other's screen |

The **desk** — windows and fences — stays daemon state, shared, last-write-wins
(ADR-0050 §2). Two people want to see the same arrangement; layout mutations are
rare enough that last-write-wins costs a window in the wrong place at worst.

The **viewport offset and the open file tabs** live in the browser. This is not
the `localStorage` fallback ADR-0050 §3 rejected: that rejection was against a
*second copy of the desk*: authoritative in no mode. This is different state
with a different lifetime, stored once. The cost is stated plainly in the
consequences: the pan does not follow the operator across machines.

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
- **The popup never writes the desk.** Its internal layout is throwaway: the
  operator lays it out for that window's shape, and the consoles return to the
  positions they left. The desk-write path is therefore reached through an
  **injected sink** — the shell passes the daemon-backed one, the popup a null
  one — and not a `detached` flag branched at each persistence call site. The
  failure mode worth designing against is one write leaking through a missed
  branch months later; an injected sink makes the popup *incapable* of writing.
- **The popup opens no consoles**, so its contents are exactly §6's snapshot and
  re-attach stays a well-defined inverse.
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
