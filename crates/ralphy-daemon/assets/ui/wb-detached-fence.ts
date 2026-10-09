// The second page script of `detached-fence.html`, the torn-off fence window.
// Importing it does nothing: `detached-fence-main.ts` calls
// `wireDetachedFence` after it created the consoles and the note cards, so
// `WBConsole` and `WBNotes` exist when the shell answers "ready" (ADR-0075
// D5, D9).
import { WBColumns } from "./wb-columns.ts";
import type { Grid } from "./wb-columns.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import type { DetachMessage } from "./wb-detach-link.ts";
import type { PopupMember } from "./wb-console-popups.ts";
import type { ConsoleWin } from "./wb-types.d.ts";

export function wireDetachedFence(window: Window, document: Document) {
  // Where this window will talk: the concrete origin, never `"*"`, as in the
  // page's own script that builds the console options.
  const PEER = window.location.origin;

  // Boot: ASK the shell for this fence and its members. They do not ride in
  // the URL, because a URL is composed by whoever sends the link — a
  // `detached-fence.html#<json>` hash would let anyone render content of
  // their choosing on the daemon's own origin, and this page is served
  // without a credential under every auth policy.
  //
  // The request goes to `window.opener` with a concrete `targetOrigin`, so a
  // page on any other origin never receives it and can never answer it. No
  // opener, or a foreign one, means no fence — and we say so rather than
  // rendering anything.
  let mounted = false;
  let fenceId: string | null = null;
  // The opener's tab identity, learned from the HANDOVER and never from
  // this document's own storage (a copy of the opener's, which drifts).
  // It scopes every message on a browser-WIDE channel: a second tab of the
  // same origin hears them all and must ignore every one of ours.
  let TAB: string | null = null;
  // This window's identity, given by the opener at the handover (#476).
  // Every lifecycle message carries it, so the opener can tell this
  // window from an earlier popup of the same fence.
  let PID: string | null = null;
  let MEMBERS: PopupMember[] = [];
  const stageEl = () => document.getElementById("stage")!;
  const empty = (why: string) => {
    stageEl().innerHTML = '<p class="detached-empty">Nothing to show. ' + why + ".</p>";
  };
  if (!window.opener) {
    empty("Detach a fence from the workbench first");
    return;
  }
  window.addEventListener("message", (e) => {
    if (e.origin !== window.location.origin) return;
    if (e.source !== window.opener) return;
    const m = e.data;
    if (!m || m.type !== "wb-fence-open" || !m.fence) return;
    if (mounted) return;
    mounted = true;
    fenceId = m.fence.id;
    TAB = m.tab ?? null;
    PID = typeof m.pid === "string" ? m.pid : null;
    // The UNTRANSLATED snapshot, kept verbatim: it is what this window
    // hands back after the opener reloads, so the consoles return to the
    // boxes they were detached from whatever was done to them here.
    MEMBERS = m.members || [];
    document.title = (m.fence.name || "fence") + " · Ralphy";
    WBConsole.mountDetached(m.fence, MEMBERS);
    addTools();
    openLink();
  });
  window.opener.postMessage({ type: "wb-fence-ready" }, PEER);
  // A shell that never answers (a foreign opener, or one that closed
  // mid-handover) leaves the plane empty rather than pending forever.
  setTimeout(() => {
    if (!mounted) empty("The workbench did not send a fence to this window");
  }, 3000);
  // Re-attach: closing this window sends the consoles home. The opener
  // also polls `handle.closed`, because a force-closed popup fires no
  // unload — and its fold makes the doubled signal a no-op.
  window.addEventListener("beforeunload", () => {
    if (window.opener && fenceId) {
      window.opener.postMessage({ type: "wb-fence-reattach", fenceId }, PEER);
    }
  });

  // ---- columns and a note on top (ADR-0051 §8, amended 2026-10-05) ----
  // The fence head's "as columns", for this window. The grid lives
  // here, as `app.ts` holds it for the shell, and is never stored: the
  // popup's layout is throwaway. `wb-console.ts` paints it.
  let grid: Grid = [];
  const consoles = () => [...stageEl().querySelectorAll<ConsoleWin>(".session-window")];
  const capNow = () => WBColumns.cap(WBConsole.columnMeasure().viewport, WBConsole.PHONE_MAX_WIDTH);
  const paintGrid = (g: Grid, opts?: { unmax?: string | null; raise?: boolean }) => {
    const cap = WBColumns.flat(g).length >= 2 ? capNow() : 1;
    WBConsole.applyColumns(WBColumns.painted(g, cap), { cap, unmax: null, raise: false, ...opts });
  };
  function openColumns() {
    const all = WBColumns.fromRects(consoles().map((w) => ({ id: w._deskId, rect: WBConsole.restoreRect(w) })));
    const ids = WBColumns.flat(all);
    if (!ids.length) return;
    const old = WBColumns.flat(grid)[0] ?? stageEl().querySelector<ConsoleWin>(".session-window.maximized")?._deskId;
    grid = ids.length >= 2 ? all : [];
    paintGrid(all, { unmax: old && !ids.includes(old) ? old : null, raise: true });
    WBConsole.focusColumn(ids[0]);
  }
  function restoreColumn(id: string) {
    const r = WBColumns.restore(grid, id);
    grid = r.ended ? [] : r.columns;
    paintGrid(r.columns, { unmax: r.unmax, raise: true });
  }
  // A closed console leaves the grid; a resize repaints it. As the
  // shell's `paintColumns`: at a cap of 1 the first console is a plain
  // maximize, and its Restore is still a column restore.
  function repaintColumns() {
    const head = WBColumns.flat(grid)[0];
    if (!head) return;
    const byId = new Map(consoles().map((w) => [w._deskId, w]));
    const headWin = byId.get(head);
    if (headWin && !headWin.classList.contains("maximized") && !headWin.classList.contains("column")) {
      return restoreColumn(head);
    }
    const kept = WBColumns.keep(grid, new Set(byId.keys()));
    grid = WBColumns.flat(kept).length >= 2 ? kept : [];
    paintGrid(kept);
  }
  document.addEventListener("workbench:column-restore", (e) => restoreColumn(e.detail.id));
  document.addEventListener("workbench:columns-stale", repaintColumns);
  document.addEventListener("workbench:consoles-changed", repaintColumns);
  window.addEventListener("resize", repaintColumns);

  // Each press keeps the next note of this window on top; the card's
  // own Put back takes it off. One card on top at a time.
  function nextNoteOnTop() {
    const ids = [...stageEl().querySelectorAll<HTMLElement>(".note-card")].map((el) => el.dataset.noteId);
    const now = window.WBNotes?.onTopNow();
    if (!ids.length || (now && ids.length === 1)) return;
    window.WBNotes.keepOnTop(ids[(ids.indexOf(now) + 1) % ids.length]);
  }

  function addTools() {
    const tools = document.createElement("div");
    tools.className = "detached-tools";
    const note = document.createElement("button");
    note.className = "detached-note";
    note.type = "button";
    note.title = "Keep a note on top";
    note.innerHTML = '<i class="bi bi-sticky-fill"></i>';
    note.addEventListener("click", nextNoteOnTop);
    const columns = document.createElement("button");
    columns.className = "detached-columns";
    columns.type = "button";
    columns.title = "Open this fence's consoles as columns";
    columns.innerHTML = '<i class="bi bi-layout-three-columns"></i>';
    columns.addEventListener("click", openColumns);
    tools.append(note, columns);
    document.body.append(tools);
    // The note button only while this window holds a card.
    const paint = () => {
      note.hidden = !stageEl().querySelector(".note-card");
    };
    new MutationObserver(paint).observe(stageEl(), { childList: true });
    paint();
  }

  // ---- the lifecycle link (issue #347) --------------------------------
  // The opener's HANDLE to this window dies with its document, so after an
  // F5 the postMessage pair above can no longer find us. The channel is
  // how the two documents re-find each other — and how this one learns
  // that its opener is gone for good.
  function openLink() {
    // `channel()`, not `link()`: this document reaches the lifecycle
    // channel and NO store at all. `window.open` handed it a COPY of its
    // opener's session storage, so every registry answer it could give is
    // a ghost that drifts the moment the real tab writes.
    const LINK = WBDetachLink.channel();
    let peer: { seen: number | null; lost: boolean } = { seen: Date.now(), lost: false };
    let closing = false;
    // Only `popup-here` carries the snapshot: it is the re-adoption
    // payload, and cloning every member record once a second would be a
    // structured clone of the whole fence at 1 Hz for nothing.
    const say = (type: string) =>
      LINK.post(
        type === "popup-here"
          ? { type, tab: TAB, fenceId, pid: PID, members: MEMBERS }
          : { type, tab: TAB, fenceId, pid: PID },
      );

    // A console CLOSED in this window ended a real daemon session. The
    // opener re-attaches from `MEMBERS`, so leaving the closed one in
    // there sends a dead window home — a box wired to a session that no
    // longer exists, which is what "connection lost" on the plane after a
    // re-attach was. Prune the snapshot to the windows this document still
    // holds and tell the opener, which drops the desk record too.
    //
    // Announced on CHANGE rather than on the beat: the operator can close
    // a console and this whole window within one heartbeat, and the last
    // thing said must already be the truth.
    //
    // A card is never pruned here: `WBConsole.list()` lists consoles
    // only, and the opener needs the note members to check a name
    // report and to bring a draft home.
    const syncMembers = () => {
      const alive = new Set(WBConsole.list().map((w) => w.id));
      const next = MEMBERS.filter((m) => m.kind === "note" || alive.has(m.id));
      if (next.length === MEMBERS.length) return;
      MEMBERS = next;
      LINK.post({ type: "popup-members", tab: TAB, fenceId, pid: PID, members: MEMBERS });
    };
    document.addEventListener("workbench:consoles-changed", syncMembers);

    // A card here gave a never-saved note its file. This document
    // cannot write the desk, so the opener records the name (ADR-0064
    // §8, amended for #475). Twice, deliberately: `postMessage` arrives
    // before this window's own `wb-fence-reattach`, and the channel
    // still reaches the opener after it reloads. `MEMBERS` learns the
    // name too, because it is what `popup-here` hands back.
    document.addEventListener("workbench:note-named", (e) => {
      const noteId = e.detail?.id;
      const path = e.detail?.path;
      if (!noteId || typeof path !== "string") return;
      MEMBERS = MEMBERS.map((m) => {
        if (m.kind !== "note" || m.id !== noteId) return m;
        const { draft, claim, ...rest } = m;
        return { ...rest, path };
      });
      if (window.opener) window.opener.postMessage({ type: "wb-note-named", noteId, path }, PEER);
      LINK.post({ type: "popup-note-named", tab: TAB, fenceId, pid: PID, noteId, path });
    });
    // The name chosen BEFORE the first write, by the same two routes. If
    // this window closes with that write in flight, the opener knows
    // which file to read on re-attach.
    document.addEventListener("workbench:note-claimed", (e) => {
      const noteId = e.detail?.id;
      const claim = e.detail?.claim;
      if (!noteId || typeof claim !== "string") return;
      MEMBERS = MEMBERS.map((m) => (m.kind === "note" && m.id === noteId ? { ...m, claim } : m));
      if (window.opener) window.opener.postMessage({ type: "wb-note-claimed", noteId, claim }, PEER);
      LINK.post({ type: "popup-note-claimed", tab: TAB, fenceId, pid: PID, noteId, claim });
    });

    // Is the opener actually GONE, or merely quiet? Silence is the
    // weakest evidence there is, because the clock that produces it is
    // the first thing a browser takes away: Chrome throttles a hidden
    // tab's timers to one tick per MINUTE after about five minutes, so a
    // workbench sitting behind another tab stops beating while it is
    // perfectly alive. Six seconds of that used to close this window out
    // from under the operator, mid-work.
    //
    // `window.opener` is the answer that owes nothing to a timer: it is
    // this document's own handle on the tab that opened it, and `closed`
    // is synchronous. Only when that handle is gone — or when a PROBE
    // goes unanswered for a whole further window, which is the case after
    // the opener navigated away rather than closed — is the opener gone.
    // Message delivery is not timer-throttled, so a merely slow opener
    // still answers.
    let probed = false;
    const openerGone = () => !window.opener || window.opener.closed;
    const silent = () => {
      if (openerGone()) return lost();
      // The handle says the tab is open, so the only way it is really
      // gone is that it navigated somewhere else. Ask; a workbench
      // answers, a stranger does not.
      if (probed) return lost();
      probed = true;
      peer = { seen: Date.now(), lost: false };
      LINK.post({ type: "popup-ping", tab: TAB, fenceId, pid: PID });
    };

    const lost = () => {
      if (closing) return;
      closing = true;
      clearInterval(timer);
      // Never simply vanish from under the operator's hands: say why, then
      // go. The consoles are already coming home in the other tab — or
      // there is no other tab left to come home to.
      stageEl().innerHTML =
        '<p class="detached-lost">The workbench window was closed. This window will close too.</p>';
      setTimeout(() => window.close(), 1500);
    };

    // The origin has taken these consoles back. Go NOW and say nothing:
    // the members are already home, and after a reload the origin holds no
    // handle to this window, so its own `handle.close()` is inert — this
    // message is the only thing that can evict a popup post-F5, and
    // without it a false peer-loss would leave two windows driving one
    // session for good.
    const evicted = () => {
      if (closing) return;
      closing = true;
      clearInterval(timer);
      window.close();
    };

    // An order with no `pid` comes from an opener that reloaded and has
    // not adopted this window yet, and it is still for this window.
    const mine = (m: DetachMessage) => m.pid == null || m.pid === PID;
    LINK.onMessage((m) => {
      if (!m || typeof m.type !== "string") return;
      if (TAB == null || m.tab !== TAB) return;
      // A window that is unloading answers nothing: its `popup-here`
      // could be the one a reloaded opener adopts, over the popup that
      // replaced it (#476).
      if (leaving) return;
      // Any word from the opener answers the probe — `silent` asks "has
      // it spoken SINCE I asked", and every branch below is speech.
      if (m.type.startsWith("origin-")) probed = false;
      if (m.type === "origin-here" || m.type === "origin-ping") {
        // Both mean "are you there" — one broadcast at the opener's boot,
        // one aimed by a glyph click. The answer is the same, and it is
        // what re-adopts this window after the opener's reload.
        peer = WBConsole.peerFold(peer, { type: "beat", at: Date.now() }, LINK.PEER_WINDOW_MS).state;
        say("popup-here");
      } else if (m.type === "origin-beat") {
        peer = WBConsole.peerFold(peer, { type: "beat", at: Date.now() }, LINK.PEER_WINDOW_MS).state;
      } else if (m.type === "origin-focus" && m.fenceId === fenceId && mine(m)) {
        window.focus();
      } else if (m.type === "origin-close" && m.fenceId === fenceId && mine(m)) {
        evicted();
      }
    });

    const timer = setInterval(() => {
      say("popup-beat");
      const out = WBConsole.peerFold(peer, { type: "tick", at: Date.now() }, LINK.PEER_WINDOW_MS);
      peer = out.state;
      if (out.effects.some((x) => x.type === "peer-lost")) silent();
    }, LINK.HEARTBEAT_MS);

    // BOTH, deliberately: `beforeunload` is the one a `window.close()` from
    // script fires reliably, `pagehide` is the one a bfcache-eligible
    // navigation fires. The opener's fold makes the doubled signal a no-op.
    let leaving = false;
    const gone = () => {
      leaving = true;
      say("popup-gone");
    };
    // A page restored from the back/forward cache is alive again.
    window.addEventListener("pageshow", (e) => {
      if (e.persisted) leaving = false;
    });
    window.addEventListener("beforeunload", gone);
    window.addEventListener("pagehide", gone);
  }
}
