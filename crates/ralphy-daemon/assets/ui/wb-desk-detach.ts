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
import { forwardAction } from "./wb-events.ts";
import { peerFold } from "./wb-desk-peer.ts";
import { createNoteReports } from "./wb-desk-detach-notes.ts";
import { isFencePopupMessage } from "./wb-desk-popup-message.ts";
import type { PopupEntry, PopupMember, PopupRegistry } from "./wb-desk-popups.ts";
import type { DetachReason } from "./wb-console-session.ts";
import type { FenceList } from "./wb-stage-fence-list.ts";
import type { DetachMessage } from "./wb-detach-link.ts";
import type { TerminalOpts } from "./wb-console-terminal-types.d.ts";
import type {
  ConsoleWin,
  DeskFence,
  DeskNote,
  DeskRecord,
  ExtentOpts,
  SpawnCarry,
  WindowSnapshot,
} from "./wb-types.d.ts";

const { fenceMembership, fenceOf } = WBGeometry;
const { detachFold, popupMatches } = WBDeskFolds;
const { sessionIdOf } = WBWindowState;

export { isFencePopupMessage } from "./wb-desk-popup-message.ts";

// The lifecycle channel of this tab, as the opener's side and the desk use it.
export type OpenerLink = {
  tab: string | null;
  post: (m: DetachMessage) => void;
  onMessage: (fn: (m: DetachMessage) => void) => void;
  // The detach registry this tab stored before a reload.
  readRegistry: () => string[];
  readMembers: () => Record<string, string[]>;
};

// What the opener's side reads from the console, and nothing else.
export type DetachDeps = {
  // The console's page: the note cards (`WBNotes`) and the window "message"
  // handshake with the popups.
  window: Window;
  // The page's origin: a handshake message from another origin is ignored.
  location: { origin: string };
  // The lifecycle channel to the popups of this tab.
  link: OpenerLink;
  // The fences this tab detached and their popup entries.
  popups: PopupRegistry;
  // The console windows on the stage.
  wins: Set<ConsoleWin>;
  // The GPU budget: a member that leaves the stage leaves its watch.
  budget: { untrackDormancy: (win: ConsoleWin) => void };
  // The fence records and the note cards; the console reassigns both lists,
  // so they are read at each use.
  fences: () => DeskFence[];
  notes: () => DeskNote[];
  // The plane; null before the page has it.
  stage: () => HTMLElement | null;
  // Tells the console that its window set changed.
  changed: () => void;
  // Grows or fits the stage to the windows on it.
  applyExtent: (opts?: ExtentOpts) => void;
  // Drops a window's desk record.
  forgetRecord: (deskId: string) => void;
  // The desk records, as a copy.
  loadDesk: () => DeskRecord[];
  // Writes the note cards to the desk.
  saveNotes: (next: DeskNote[]) => void;
  // Opens a console window over a live session.
  spawnWindow: (
    termOpts: TerminalOpts,
    label: string,
    repo: string | null | undefined,
    desk?: SpawnCarry,
  ) => ConsoleWin;
  // Opens a placeholder window for a record with no session.
  spawnPlaceholder: (record: SpawnCarry) => ConsoleWin;
  // A window's desk record, as the stage has it now.
  deskOf: (win: ConsoleWin) => WindowSnapshot;
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
  const { replayNoteNames, recordNoteName, recordNoteClaim } = createNoteReports({
    popups,
    notes,
    saveNotes,
    whenDeskLoaded,
  });
  const { readFenceRects, readWindowRects, refreshFenceChrome, renderNotes, showDetachGlyph } =
    fenceFloor;

  // ---- detaching a fence into its own window (issues #346, #347) ---------------
  // The popup registry, `detached` and `fencePopups`, is `popups`
  // (`wb-desk-popups.ts`): every change to either goes through it.
  const PEER_WINDOW = WBDetachLink.PEER_WINDOW_MS;
  const HEARTBEAT = WBDetachLink.HEARTBEAT_MS;

  // The popup's member set, adopted as the truth. A console CLOSED inside the
  // popup ended a real daemon session, so its desk RECORD goes too. Both
  // `popup-members` and `popup-here` arrive here, so they never prune differently.
  function adoptMembers(id: string, members: DetachMessage["members"]) {
    const entry = popups.entry(id);
    if (!entry || !Array.isArray(members)) return;
    const alive = new Set(members.map((m) => m.id).filter(Boolean));
    const dropped = [...entry.members.map((m) => m.id), ...entry.memberIds].filter(
      (wid) => wid !== "" && !alive.has(wid),
    );
    entry.members = members;
    entry.memberIds = members.map((m) => m.id).filter(Boolean);
    entry.adopted = true;
    // A remove is a change the daemon applies to its own desk, so a record
    // this page has not read yet goes too.
    for (const wid of new Set(dropped)) forgetRecord(wid);
    commitDetached(popups.detachedIds());
  }

  // Is a quiet peer actually GONE? Silence is weak evidence: Chrome throttles
  // a hidden tab's timers to one tick per MINUTE after ~5 minutes, so a
  // workbench behind another tab stops beating while alive. Two better
  // witnesses, in order: the WINDOW HANDLE (answers `closed` synchronously,
  // when this document opened the popup), then a PROBE (message delivery is
  // not throttled; one unheard probe is death). A handle-less entry (this tab
  // reloaded) has only the second.
  function stillThere(id: string, entry: PopupEntry) {
    if (entry.handle && !entry.handle.closed) {
      entry.peer = { seen: Date.now(), lost: false };
      entry.probed = false;
      return true;
    }
    if (entry.probed === true) return false;
    entry.probed = true;
    entry.peer = { seen: Date.now(), lost: false };
    link.post({ type: "origin-ping", tab: link.tab, fenceId: id });
    return true;
  }

  // The origin's heartbeat: one interval for ALL entries, so the cost does not
  // scale with the cap. It both announces this tab and ages every peer.
  let beat: ReturnType<typeof setInterval> | null = null;
  function startBeat() {
    if (beat !== null) return;
    beat = setInterval(() => {
      link.post({ type: "origin-beat", tab: link.tab });
      const at = Date.now();
      // A copy: `reattachFence` removes an entry inside this loop.
      for (const [id, entry] of popups.entries()) {
        const out = peerFold(entry.peer, { type: "tick", at }, PEER_WINDOW);
        entry.peer = out.state;
        // Consoles must never be nowhere: a popup that stopped answering brings
        // its members home. But SILENCE IS NOT DEATH (`stillThere`).
        if (out.effects.length > 0 && !stillThere(id, entry)) {
          reattachFence(id);
        }
      }
    }, HEARTBEAT);
  }
  function stopBeat() {
    if (beat !== null) clearInterval(beat);
    beat = null;
  }

  // Whether this tab hears `m`. The channel is browser-WIDE: the `tab` filter
  // keeps a SECOND tab's popups out, the `origin-` prefix drop keeps this tab
  // from consuming its own broadcasts.
  function fromThisTab(m: DetachMessage) {
    if (link.tab == null || m.tab !== link.tab) return false;
    return !m.type.startsWith("origin-");
  }

  // An EARLIER popup of the same fence still talks while it unloads: its
  // `popup-gone` would re-attach the popup that replaced it (#476). Only the
  // popup this entry holds is heard. A ping is answered whoever sends it.
  function heardFrom(m: DetachMessage) {
    if (!fromThisTab(m)) return false;
    if (m.type === "popup-here" || m.type === "popup-ping") return true;
    if (m.fenceId == null) return false;
    return popupMatches(popups.entry(m.fenceId), m);
  }

  // The fence record of a popup entry: the one on the stage, or an empty one.
  function fenceFor(id: string) {
    const st = stage();
    return (
      (st ? readFenceRects(st).find((f) => f.id === id) : null) ?? { id, name: "", rect: null }
    );
  }

  // A popup that survived this tab's reload, announcing which fence it
  // holds. Adopted only when the RESTORED registry already says that fence
  // is detached — the payload alone must never be able to detach one.
  function onPopupHere(id: string, m: DetachMessage) {
    if (!isDetached(id)) return;
    if (popups.has(id) && !popupMatches(popups.entry(id), m)) return;
    // MUTATED IN PLACE, never replaced: `glyphClick`'s ping compares entries.
    const entry = popups.entry(id) ?? newPopupEntry();
    // A restored entry learns which popup it holds from its first answer.
    if (entry.pid == null && typeof m.pid === "string") entry.pid = m.pid;
    entry.greeted = true;
    // The popup hands back the UNTRANSLATED snapshot it was given. Adopted
    // whole, EMPTY included: an empty set is an answer, not a missing one.
    popups.put(id, entry);
    if (Array.isArray(m.members)) {
      adoptMembers(id, m.members);
      replayNoteNames(id, m.members);
    }
    entry.fence ??= fenceFor(id);
    entry.peer = { seen: Date.now(), lost: false };
    entry.probed = false;
    popups.put(id, entry);
    // Re-persist: the snapshot the popup handed back is a better member list
    // than the ids this tab restored, and the NEXT reload reads it.
    commitDetached(popups.detachedIds());
    showDetachGlyph(id, true);
  }

  function onPopupBeat(id: string) {
    const entry = popups.entry(id);
    if (entry?.peer) {
      entry.peer = peerFold(entry.peer, { type: "beat", at: Date.now() }, PEER_WINDOW).state;
    }
    // Any word clears the probe: `stillThere` asks "has it answered SINCE I
    // asked", and a beat is an answer.
    if (entry) entry.probed = false;
  }

  // A console closed INSIDE the popup. Same registry gate as `popup-here`.
  function onPopupMembers(id: string, members: DetachMessage["members"]) {
    if (isDetached(id)) adoptMembers(id, members);
  }

  // The popup asking whether THIS document is still here. Answering from a
  // message handler is the point: a throttled tab still delivers messages.
  function onPopupPing(id: string) {
    if (isDetached(id)) link.post({ type: "origin-here", tab: link.tab, fenceId: id });
  }

  // The origin's half of the lifecycle channel.
  link.onMessage((m) => {
    if (!heardFrom(m)) return;
    // A message with no fence id changes nothing: no entry answers to it.
    const id = m.fenceId;
    if (id == null) return;
    if (m.type === "popup-here") {
      onPopupHere(id, m);
    } else if (m.type === "popup-members") {
      onPopupMembers(id, m.members);
    } else if (m.type === "popup-beat") {
      onPopupBeat(id);
    } else if (m.type === "popup-ping") {
      onPopupPing(id);
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

  // Unique in this browser: the clock separates this tab's documents, the
  // counter separates two detaches in one millisecond.
  let popupSeq = 0;
  function newPid() {
    popupSeq += 1;
    return `${Date.now().toString(36)}-${popupSeq}`;
  }

  // What the popup is handed: one record per member in the shape `buildChrome`
  // restores from, plus the live session id. Rects are measured HERE,
  // untranslated, so a re-attach returns every console to its original box.
  function fenceSnapshot(id: string): PopupMember[] {
    const st = stage();
    if (!st) return [];
    // The stage's windows only: a member in its own tab stays in the tab, and
    // its shadow stays in the fence (ADR-0037 §3d).
    const all = [...st.querySelectorAll<ConsoleWin>(".session-window")];
    const byId = new Map(all.map((w): [string, ConsoleWin] => [w._deskId, w]));
    const ids = fenceMembership(readFenceRects(st), readWindowRects(st))[id] ?? [];
    const windows: PopupMember[] = ids
      .map((wid) => byId.get(wid))
      .filter(Boolean)
      .map((win) => ({ ...deskOf(win), session: sessionIdOf(win) }));
    // The cards the fence holds ride along (ADR-0064 §8), tagged so the popup
    // and the re-attach can tell them from a console. Their RECORDS travel,
    // not their DOM. A note with no file yet carries its unsaved text
    // (`draft`), in this snapshot only, never in the desk (#475).
    const cards = notes()
      .filter((n) => fenceOf(fences(), n.rect)?.id === id)
      .map((n): PopupMember => ({ ...n, ...window.WBNotes.draftOf(n.id), kind: "note" }));
    return windows.concat(cards);
  }

  // Take a member off the plane WITHOUT forgetting its desk record (a second
  // client still renders it) and WITHOUT closing its daemon session:
  // `dispose()` closing the socket releases the writer slot the popup then
  // re-acquires (ADR-0051 §9).
  function tearDownMember(win: ConsoleWin, reason: DetachReason) {
    win._term?.dispose(reason);
    win.remove();
    budget.untrackDormancy(win);
    wins.delete(win);
    changed();
  }

  function stopPoll(entry: PopupEntry | undefined) {
    if (entry?.poll != null) clearInterval(entry.poll);
    if (entry?.rescue != null) clearTimeout(entry.rescue);
  }

  // Closes the popup of fence `id`, by its handle when this document has one.
  function closePopup(id: string, entry: PopupEntry | undefined) {
    try {
      if (entry?.handle && !entry.handle.closed) entry.handle.close();
    } catch {}
    // After a reload the handle is null, so only the channel can evict the popup.
    link.post({ type: "origin-close", tab: link.tab, fenceId: id, pid: entry?.pid ?? undefined });
  }

  // A card comes home by RE-RENDER: its record never left the desk. A draft
  // whose popup closed before its first save comes home with it.
  function homeNote(m: PopupMember) {
    const record = notes().find((n) => n.id === m.id);
    if (typeof m.draft === "string" && record && (record.path ?? "") === "") {
      window.WBNotes.adoptDraft(m.id, m.draft, m.claim);
    }
  }

  // A console member comes home as a window over its session, or as a
  // placeholder.
  function homeConsole(m: PopupMember) {
    // A member already on the plane is not re-spawned: two windows over one
    // session is worse than a console left away.
    if (m.id && [...wins].some((w) => w._deskId === m.id)) return;
    // The name comes from the desk, not the snapshot: a rename on another page
    // while the fence was away must not be undone (ADR-0066 §3).
    const rec = loadDesk().find((r) => r.id === m.id);
    const name = rec?.consoleName ?? "";
    const member = name ? { ...m, consoleName: name } : m;
    if (member.session != null) {
      spawnWindow(
        { id: member.session, repo: member.repo },
        (member.agent ?? "") || "console",
        member.repo,
        member,
      );
    } else {
      // The snapshot keeps no session id, so a member relaunched in the
      // popup comes home as a placeholder. `_revive` attaches it to the
      // session that runs now and never launches one.
      void spawnPlaceholder(member)._revive?.();
    }
  }

  // `opts.force` is the GLYPH's call only. The automatic paths (`beforeunload`,
  // the closed-poll, the peer-loss tick) stay gated on the fold because their
  // signals arrive DOUBLED. A click must land even when the state behind the
  // glyph is wrong; forcing is safe because the entry is deleted here.
  function reattachFence(id: string, opts: { force?: boolean } = {}) {
    const out = detachFold(popups.detachedIds(), { type: "reattach", fenceId: id });
    const held = out.effects.some((e) => e.type === "close");
    if (!held && opts.force !== true) return;
    const entry = popups.entry(id);
    stopPoll(entry);
    popups.remove(id);
    commitDetached(out.registry);
    closePopup(id, entry);
    // The ORIGINAL records: the popup's own layout is never read.
    for (const m of entry?.members ?? []) {
      if (m.kind === "note") homeNote(m);
      else homeConsole(m);
    }
    showDetachGlyph(id, false);
    renderNotes();
    applyExtent();
    refreshFenceChrome();
    WB.emit("fence-reattach", { fence: id });
  }

  // The glyph is ONE verb: bring these consoles home, whatever the registry
  // believes. `origin-close` is idempotent, so the handle and the channel may
  // both close the window.
  function glyphClick(id: string) {
    reattachFence(id, { force: true });
  }

  // The opener's half of the handshake: answered only from this origin AND
  // from a window this tab itself opened.
  window.addEventListener("message", (e) => {
    if (e.origin !== location.origin) return;
    const owner = popupOwner(e.source);
    if (owner == null) return;
    const m: unknown = e.data;
    if (!isFencePopupMessage(m)) return;
    if (m.type === "wb-fence-ready") {
      greetPopup(owner, e.source as Window);
    } else if (m.type === "wb-emit") {
      forwardAction(WB, m.action, m.detail);
    } else if (m.type === "wb-note-named") {
      recordNoteName(owner, m);
    } else if (m.type === "wb-note-claimed") {
      recordNoteClaim(owner, m);
    } else {
      // "wb-fence-reattach", the last type of the message.
      // `owner`, never the message's own field: the source lookup PROVED which
      // fence this window holds; the payload could name any.
      reattachFence(owner);
    }
  });

  // The fence whose popup is the window `source`, or null for a window this
  // tab did not open. The last entry that holds it wins.
  function popupOwner(source: MessageEventSource | null) {
    let owner: string | null = null;
    for (const [id, entry] of popups.entries()) if (entry.handle === source) owner = id;
    return owner;
  }

  // The popup of the fence `owner` is ready: its rescue stops, and it is
  // handed the fence and its members.
  function greetPopup(owner: string, source: Window) {
    const entry = popups.entry(owner);
    if (!entry) return;
    entry.greeted = true;
    if (entry.rescue !== null) {
      clearTimeout(entry.rescue);
      entry.rescue = null;
    }
    // `tab` rides the handover, never the popup's own storage: `window.open` gave
    // it a COPY of ours.
    source.postMessage(
      {
        type: "wb-fence-open",
        fence: entry.fence,
        members: entry.members,
        tab: link.tab,
        pid: entry.pid,
      },
      location.origin,
    );
  }

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
