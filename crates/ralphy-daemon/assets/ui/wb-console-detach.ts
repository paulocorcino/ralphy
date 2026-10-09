/* ---------------------------------------------------------------------------
   The opener's side of a detached fence: the heartbeat that ages every
   popup, the probe of a quiet popup, the re-attach that brings a popup's
   members home, and the two listeners that hear the popups (the lifecycle
   channel and the window "message" handshake).

   `createDetach(deps)` returns the functions the console, its fences and the
   `WBConsole` API use (ADR-0075 D7). They read the console only through
   `deps`, and `DetachDeps` lists every read, so `tsc` refuses a read outside
   it. `wb-console.ts` creates one per console, after its popup registry, its
   fence list and its GPU budget, and before its fences. The popup's own side
   is `wb-detached-fence.ts`.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import * as WBDeskFolds from "./wb-desk-folds.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { WBDetachLink } from "./wb-detach-link.ts";
import type { PopupRegistry } from "./wb-console-popups.ts";
import type { FenceList } from "./wb-console-fence-list.ts";

const { fenceMembership, fenceOf } = WBGeometry;
const { detachFold, popupMatches, noteNameOk } = WBDeskFolds;
const { sessionIdOf } = WBWindowState;

// What the opener's side reads from the console, and nothing else.
export type DetachDeps = {
  // The console's page: the note cards (`WBNotes`) and the window "message"
  // handshake with the popups.
  window: any;
  // The page's origin: a handshake message from another origin is ignored.
  location: { origin: string };
  // The lifecycle channel to the popups of this tab.
  link: {
    tab: any;
    post: (m: any) => void;
    onMessage: (fn: (m: any) => void) => void;
  };
  // The fences this tab detached and their popup entries.
  popups: PopupRegistry;
  // The console windows on the stage.
  wins: Set<any>;
  // The GPU budget: a member that leaves the stage leaves its watch.
  budget: { untrackDormancy: (win: any) => void };
  // The fence records and the note cards; the console reassigns both lists,
  // so they are read at each use.
  fences: () => any[];
  notes: () => any[];
  // The plane; null before the page has it.
  stage: () => any;
  // Tells the console that its window set changed.
  changed: () => void;
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: any) => void;
  // Drops a window's desk record.
  forgetRecord: (deskId: any) => void;
  // The desk records, as a copy.
  loadDesk: () => any[];
  // Writes the note cards to the desk.
  saveNotes: (next: any) => void;
  // Opens a console window over a live session.
  spawnWindow: (termOpts: any, label: any, repo: any, desk?: any) => any;
  // Opens a placeholder window for a record with no session.
  spawnPlaceholder: (record: any, missing?: any, refused?: any, held?: any) => any;
  // A window's desk record, as the stage has it now.
  deskOf: (win: any) => any;
  // Resolves once the boot desk load has settled.
  whenDeskLoaded: () => Promise<unknown>;
  // The fence list: the fences and windows as the stage has them now, the
  // fence chrome, the render of the cards and the detach glyph.
  fenceFloor: FenceList;
};

export function createDetach(deps: DetachDeps) {
  const {
    window,
    location,
    link,
    popups,
    wins,
    budget,
    fences,
    notes,
    stage,
    changed,
    applyExtent,
    forgetRecord,
    loadDesk,
    saveNotes,
    spawnWindow,
    spawnPlaceholder,
    deskOf,
    whenDeskLoaded,
    fenceFloor,
  } = deps;
  const { isDetached, commitDetached, newPopupEntry } = popups;
  const { readFenceRects, readWindowRects, refreshFenceChrome, renderNotes, showDetachGlyph } = fenceFloor;

  // A PEER's liveness as a rule: (state, event, windowMs) -> { state, effects }.
  // Pure, so it decides both directions of the link (origin watching popup,
  // popup watching origin) and the node table drives the boundary. `windowMs`
  // is an ARGUMENT, never a global.
  //
  // Loss is TERMINAL: a beat after `lost` does not resurrect — the caller turns
  // the effect into a `window.close()` or a `reattachFence`, neither undoable.
  // Loss is STRICT (`> windowMs`), so the boundary tick is still alive.
  function peerFold(state: any, event: any, windowMs: any) {
    const seen = typeof state?.seen === "number" ? state.seen : null;
    const lost = !!state?.lost;
    const same = { seen, lost };
    if (lost) return { state: same, effects: [] };
    switch (event?.type) {
      case "beat":
        return { state: { seen: typeof event.at === "number" ? event.at : seen, lost: false }, effects: [] };
      case "tick":
        // `seen: null` — never heard from — expires nothing: the origin seeds
        // every entry with `Date.now()` at creation AND at boot-restore, so one
        // rule governs the post-reload adoption grace and steady state alike.
        if (seen == null || typeof event.at !== "number") return { state: same, effects: [] };
        if (event.at - seen > windowMs)
          return { state: { seen, lost: true }, effects: [{ type: "peer-lost" }] };
        return { state: same, effects: [] };
      case "gone":
        // An announced departure: the same effect, without waiting the window out.
        return { state: { seen, lost: true }, effects: [{ type: "peer-lost" }] };
      default:
        return { state: same, effects: [] };
    }
  }

  // ---- detaching a fence into its own window (issues #346, #347) ---------------
  // The popup registry, `detached` and `fencePopups`, is `popups`
  // (`wb-console-popups.ts`): every change to either goes through it.
  const PEER_WINDOW = WBDetachLink.PEER_WINDOW_MS;
  const HEARTBEAT = WBDetachLink.HEARTBEAT_MS;

  // The popup's member set, adopted as the truth. A console CLOSED inside the
  // popup ended a real daemon session; re-attaching it would wire a window to a
  // gone session, so its desk RECORD goes too. Both `popup-members` and
  // `popup-here` arrive here, so the two never prune differently.
  function adoptMembers(id: any, members: any) {
    const entry = popups.entry(id);
    if (!entry || !Array.isArray(members)) return;
    const alive = new Set(members.map((m) => m?.id).filter(Boolean));
    const dropped = [...(entry.members || []).map((m: any) => m?.id), ...(entry.memberIds || [])].filter(
      (wid) => wid && !alive.has(wid),
    );
    entry.members = members;
    entry.memberIds = members.map((m) => m?.id).filter(Boolean);
    entry.adopted = true;
    // A remove is a change the daemon applies to its own desk, so a record
    // this page has not read yet goes too.
    for (const wid of new Set(dropped)) forgetRecord(wid);
    commitDetached(popups.detachedIds());
  }

  // Is a quiet peer actually GONE? Silence is weak evidence: LIMIT — Chrome
  // throttles a hidden tab's timers to one tick per MINUTE after ~5 minutes, so
  // a workbench behind another tab stops beating while alive, and a six-second
  // window read a working popup as dead.
  // Two better witnesses, in order: the WINDOW HANDLE (answers `closed`
  // synchronously, when this document opened the popup), then a PROBE (message
  // delivery is not throttled; one unheard probe is death). A handle-less
  // entry (this tab reloaded) has only the second.
  function stillThere(id: any, entry: any) {
    if (entry.handle && !entry.handle.closed) {
      entry.peer = { seen: Date.now(), lost: false };
      entry.probed = false;
      return true;
    }
    if (entry.probed) return false;
    entry.probed = true;
    entry.peer = { seen: Date.now(), lost: false };
    link.post({ type: "origin-ping", tab: link.tab, fenceId: id });
    return true;
  }

  // The origin's heartbeat: one interval for ALL entries, so the cost does not
  // scale with the cap. It both announces this tab and ages every peer.
  let beat: any = null;
  function startBeat() {
    if (beat) return;
    beat = setInterval(() => {
      link.post({ type: "origin-beat", tab: link.tab });
      const at = Date.now();
      // A copy: `reattachFence` removes an entry inside this loop.
      for (const [id, entry] of popups.entries()) {
        if (!entry.peer) continue;
        const out = peerFold(entry.peer, { type: "tick", at }, PEER_WINDOW);
        entry.peer = out.state;
        // Consoles must never be nowhere: a popup that stopped answering brings
        // its members home. But SILENCE IS NOT DEATH (`stillThere`).
        if (out.effects.some((e) => e.type === "peer-lost") && !stillThere(id, entry)) {
          reattachFence(id);
        }
      }
    }, HEARTBEAT);
  }
  function stopBeat() {
    if (beat) clearInterval(beat);
    beat = null;
  }

  // The origin's half of the lifecycle channel. The channel is browser-WIDE:
  // the `tab` filter keeps a SECOND tab's popups out of this registry, the
  // `origin-` prefix drop keeps this tab from consuming its own broadcasts.
  link.onMessage((m: any) => {
    if (!m || typeof m.type !== "string") return;
    if (link.tab == null || m.tab !== link.tab) return;
    if (m.type.startsWith("origin-")) return;
    const id = m.fenceId;
    // An EARLIER popup of the same fence still talks while it unloads: its
    // `popup-gone` would re-attach the popup that replaced it, and its
    // `popup-members` would drop that popup's consoles (#476). Only the popup
    // this entry holds is heard. A ping is answered whoever sends it.
    if (m.type !== "popup-here" && m.type !== "popup-ping" && !popupMatches(popups.entry(id), m)) {
      return;
    }
    if (m.type === "popup-here") {
      // A popup that survived this tab's reload, announcing which fence it
      // holds. Adopted only when the RESTORED registry already says that fence
      // is detached — the payload alone must never be able to detach one.
      if (!isDetached(id)) return;
      if (popups.has(id) && !popupMatches(popups.entry(id), m)) return;
      // MUTATED IN PLACE, never replaced: `glyphClick`'s ping compares the entry
      // it captured with the one in the map.
      const entry = popups.entry(id) || newPopupEntry();
      // A restored entry learns which popup it holds from its first answer.
      if (entry.pid == null && typeof m.pid === "string") entry.pid = m.pid;
      const st = stage();
      entry.greeted = true;
      // The popup hands back the UNTRANSLATED snapshot it was given, so a
      // re-attach puts every console back where it was detached from. Adopted
      // whole, EMPTY included: an empty set is an answer, not a missing one.
      popups.put(id, entry);
      if (Array.isArray(m.members)) {
        adoptMembers(id, m.members);
        // A name report sent while this tab was reloading reached nobody. The
        // popup's members carry the name it gave, so it is recorded from
        // here, once the desk has loaded. `recordNoteName` refuses a record
        // that already has its path, so this is safe to repeat.
        whenDeskLoaded().then(() => {
          for (const x of m.members) {
            if (x?.kind === "note" && typeof x.path === "string") {
              recordNoteName(id, { noteId: x.id, path: x.path });
            }
          }
        });
      }
      if (!entry.fence) {
        entry.fence = (st ? readFenceRects(st).find((f) => f.id === id) : null) || {
          id,
          name: "",
          rect: null,
        };
      }
      entry.peer = { seen: Date.now(), lost: false };
      entry.probed = false;
      popups.put(id, entry);
      // Re-persist: the snapshot the popup just handed back is a better member
      // list than the ids this tab restored, and the NEXT reload reads it.
      commitDetached(popups.detachedIds());
      showDetachGlyph(id, true);
    } else if (m.type === "popup-members") {
      // A console closed INSIDE the popup. Same registry gate as `popup-here`.
      if (isDetached(id)) adoptMembers(id, m.members);
    } else if (m.type === "popup-beat") {
      const entry = popups.entry(id);
      if (entry?.peer) entry.peer = peerFold(entry.peer, { type: "beat", at: Date.now() }, PEER_WINDOW).state;
      // Any word at all clears the probe: `stillThere` asks "has it answered
      // SINCE I asked", and a beat is an answer.
      if (entry) entry.probed = false;
    } else if (m.type === "popup-ping") {
      // The popup asking whether THIS document is still here. Answering from a
      // message handler is the point: a throttled tab still delivers messages.
      if (isDetached(id)) link.post({ type: "origin-here", tab: link.tab, fenceId: id });
    } else if (m.type === "popup-note-named") {
      recordNoteName(id, m);
    } else if (m.type === "popup-note-claimed") {
      recordNoteClaim(id, m);
    } else if (m.type === "popup-gone") {
      // The tab filter proved the sender is ours; `detachFold` makes a re-attach
      // of a fence this tab does not hold a no-op.
      reattachFence(id);
    }
  });

  // The popup's card gave a never-saved note its file. The popup cannot write
  // the desk (ADR-0051 §8), so it reports the name and this tab records it,
  // after checking the report. The report comes twice: over `postMessage`,
  // which arrives before the popup's own `wb-fence-reattach`, and over the
  // channel, which still reaches this tab after a reload. The second one is
  // refused because the record already has its path.
  function recordNoteName(id: any, m: any) {
    const entry = popups.entry(id);
    const record = notes().find((n: any) => n.id === m.noteId);
    if (!isDetached(id) || !noteNameOk(entry, record, m)) return;
    saveNotes(notes().map((n: any) => (n.id === m.noteId ? { ...n, path: m.path } : n)));
    // `noteNameOk` is false for an entry that does not exist.
    entry!.members = entry!.members.map((x: any) => {
      if (x.kind !== "note" || x.id !== m.noteId) return x;
      const { draft, claim, ...rest } = x;
      return { ...rest, path: m.path };
    });
  }

  // The name the popup's card chose BEFORE its first write, kept on the member
  // and never on the desk (a path on the desk says a file is there). If the
  // popup closes with that write in flight, no name report follows, and a
  // re-attach reads or writes this name instead of choosing a second one.
  function recordNoteClaim(id: any, m: any) {
    const entry = popups.entry(id);
    const record = notes().find((n: any) => n.id === m.noteId);
    if (!isDetached(id) || !noteNameOk(entry, record, { noteId: m.noteId, path: m.claim })) return;
    entry!.members = entry!.members.map((x: any) =>
      x.kind === "note" && x.id === m.noteId ? { ...x, claim: m.claim } : x,
    );
  }

  // Unique in this browser: the clock separates this tab's documents, the
  // counter separates two detaches in one millisecond.
  let popupSeq = 0;
  function newPid() {
    popupSeq += 1;
    return `${Date.now().toString(36)}-${popupSeq}`;
  }

  // What the popup is handed: one record per member in the shape `buildChrome`
  // restores from, plus the live session id. Rects are measured HERE,
  // untranslated — the popup translates for its own viewport and never sends
  // them back, so a re-attach returns every console to its original box.
  function fenceSnapshot(id: any) {
    const st = stage();
    if (!st) return [];
    const all = [...st.querySelectorAll(".session-window")];
    const byId = new Map(all.map((w) => [w._deskId, w]));
    const ids = fenceMembership(readFenceRects(st), readWindowRects(st))[id] || [];
    const windows = ids
      .map((wid: any) => byId.get(wid))
      .filter(Boolean)
      .map((win: any) => ({
        ...deskOf(win),
        session: sessionIdOf(win),
      }));
    // The cards the fence holds ride along (ADR-0064 §8), tagged so the popup
    // and the re-attach can tell them from a console. Their RECORDS travel,
    // not their DOM: a card is rebuilt in the popup from the same desk record
    // the stage built it from.
    // A note with no file yet carries its unsaved text (`draft`): the popup
    // must open in this click, before a first save could land. The draft
    // lives in this snapshot only, never in the desk (ADR-0064 §8, #475).
    const cards = notes()
      .filter((n: any) => fenceOf(fences(), n.rect || {})?.id === id)
      .map((n: any) => ({ ...n, ...window.WBNotes?.draftOf?.(n.id), kind: "note" }));
    return windows.concat(cards);
  }

  // Take a member off the plane WITHOUT forgetting its desk record (shared
  // state a second client still renders) and WITHOUT closing its daemon
  // session: `dispose()` closing the socket is the writer-slot release the
  // popup then re-acquires (ADR-0051 §9).
  function tearDownMember(win: any, reason: any) {
    win._term?.dispose(reason);
    win.remove();
    budget.untrackDormancy(win);
    wins.delete(win);
    changed();
  }

  function stopPoll(entry: any) {
    if (entry?.poll) clearInterval(entry.poll);
    if (entry?.rescue) clearTimeout(entry.rescue);
  }

  // `opts.force` is the GLYPH's call only. The automatic paths (`beforeunload`,
  // the closed-poll, the peer-loss tick) stay gated on the fold because their
  // signals arrive DOUBLED. A click is an instruction that must land even when
  // the state behind the glyph is wrong. Forcing is safe against the doubled
  // signal for the same reason the fold is: the entry is deleted here.
  function reattachFence(id: any, opts: any = {}) {
    const out = detachFold(popups.detachedIds(), { type: "reattach", fenceId: id });
    const held = out.effects.some((e) => e.type === "close");
    if (!held && !opts.force) return;
    const entry = popups.entry(id);
    stopPoll(entry);
    popups.remove(id);
    commitDetached(out.registry);
    try {
      if (entry?.handle && !entry.handle.closed) entry.handle.close();
    } catch {}
    // After a reload the handle is null, so only the channel can evict the
    // popup; otherwise it keeps driving the sessions re-spawned here.
    link.post({ type: "origin-close", tab: link.tab, fenceId: id, pid: entry?.pid ?? undefined });
    // The ORIGINAL records: the popup's own layout is discarded by never having
    // been read.
    for (const m of entry?.members || []) {
      // A card comes home by RE-RENDER: its record never left the desk, and
      // `renderNotes` puts back every card whose fence is no longer detached.
      // A draft whose popup closed before its first save comes home with it.
      if (m.kind === "note") {
        const record = notes().find((n: any) => n.id === m.id);
        if (typeof m.draft === "string" && record && !record.path) {
          window.WBNotes?.adoptDraft?.(m.id, m.draft, m.claim);
        }
        continue;
      }
      // A member already on the plane is not re-spawned: two windows over one
      // session is worse than a console left away.
      if (m.id && [...wins].some((w) => w._deskId === m.id)) continue;
      // The name comes from the desk, not the snapshot: a rename on another page
      // while the fence was away must not be undone (ADR-0066 §3).
      const rec = loadDesk().find((r: any) => r.id === m.id);
      const member = rec?.consoleName ? { ...m, consoleName: rec.consoleName } : m;
      if (member.session != null) {
        spawnWindow({ id: member.session, repo: member.repo }, member.agent || "console", member.repo, member);
      } else {
        // The snapshot keeps no session id, so a member relaunched in the
        // popup comes home as a placeholder. `_revive` attaches it to the
        // session that runs now and never launches one.
        spawnPlaceholder(member)._revive();
      }
    }
    showDetachGlyph(id, false);
    renderNotes();
    applyExtent();
    refreshFenceChrome();
    WB.emit("fence-reattach", { fence: id });
  }

  // The glyph is ONE verb: bring these consoles home, whatever the registry
  // believes (raising a buried popup is the head's detach button). Close the
  // window by handle or by channel — both is fine, `origin-close` is
  // idempotent — and put the members back.
  function glyphClick(id: any) {
    reattachFence(id, { force: true });
  }

  // The opener's half of the handshake, guarded as `app.ts` guards the detached
  // FILE viewer's: answered only from this origin AND from a window this tab
  // itself opened.
  window.addEventListener("message", (e: any) => {
    if (e.origin !== location.origin) return;
    let owner = null;
    for (const [id, entry] of popups.entries()) if (entry.handle === e.source) owner = id;
    if (owner == null) return;
    const m = e.data;
    if (!m) return;
    if (m.type === "wb-fence-ready") {
      const entry = popups.entry(owner)!;
      entry.greeted = true;
      if (entry.rescue) {
        clearTimeout(entry.rescue);
        entry.rescue = null;
      }
      // `tab` rides the handover, never the popup's own storage — `window.open` gave it a COPY of ours.
      e.source.postMessage(
        { type: "wb-fence-open", fence: entry.fence, members: entry.members, tab: link.tab, pid: entry.pid },
        location.origin,
      );
    } else if (m.type === "wb-emit") {
      WB.emit(m.action, m.detail);
    } else if (m.type === "wb-note-named") {
      recordNoteName(owner, m);
    } else if (m.type === "wb-note-claimed") {
      recordNoteClaim(owner, m);
    } else if (m.type === "wb-fence-reattach") {
      // `owner`, never the message's own field: the source lookup PROVED which
      // fence this window holds; the payload could name any.
      reattachFence(owner);
    }
  });

  return {
    peerFold,
    startBeat,
    stopBeat,
    newPid,
    fenceSnapshot,
    tearDownMember,
    reattachFence,
    glyphClick,
  };
}

export type Detach = ReturnType<typeof createDetach>;
