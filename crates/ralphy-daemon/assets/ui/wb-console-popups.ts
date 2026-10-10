/* ---------------------------------------------------------------------------
   The popup registry: the fences this tab detached into their own windows,
   and one entry per popup (ADR-0075 D7, #610).

   `createPopupRegistry(deps)` owns `detached` (the fence ids) and
   `fencePopups` (fence id -> popup entry), and has every call that changes
   them: `put` and `remove` change an entry, and `commitDetached` is the one
   place the ids change. The fences, the desk restore and the lifecycle
   channel of `wb-console.ts` read them through `isDetached`, `detachedIds`,
   `entry`, `has`, `size`, `entries` and `detachedMembers`. The popup entries
   are objects the console changes in place: a popup's handle, members and
   peer state belong to the entry, and only which entries exist belongs here.
   --------------------------------------------------------------------------- */

import type { Rect } from "./wb-types.d.ts";

// One member a popup holds: a console's record or a note card's, as the
// snapshot `fenceSnapshot` hands over. A card is tagged `kind: "note"`, and a
// never-saved one carries its unsaved text in `draft` and its chosen name in
// `claim`.
export type PopupMember = {
  id: string;
  kind?: string | null;
  agent?: string | null;
  repo?: string | null;
  consoleName?: string | null;
  path?: string;
  session?: number | null;
  rect?: Partial<Rect>;
  draft?: string;
  claim?: string | null;
};

// One popup of the registry. `pid` is the popup's identity on every lifecycle
// message, set when this tab opened it; `adopted` and `probed` start false.
export type PopupEntry = {
  handle: Window | null;
  members: PopupMember[];
  memberIds: string[];
  fence: { id: string; name?: string; rect: Rect | null } | null;
  pid?: string;
  poll: ReturnType<typeof setInterval> | null;
  greeted: boolean;
  rescue: ReturnType<typeof setTimeout> | null;
  adopted?: boolean;
  peer: { seen: number | null; lost: boolean };
  probed?: boolean;
};

// What the registry reads from the console, and nothing else.
export type PopupRegistryDeps = {
  // The session-scoped store of this tab's registry.
  link: { writeRegistry: (ids: string[], members: Record<string, string[]>) => void };
  // The origin's heartbeat: it runs while a fence is detached.
  startBeat: () => void;
  stopBeat: () => void;
};

export function createPopupRegistry(deps: PopupRegistryDeps) {
  const { link, startBeat, stopBeat } = deps;

  // `detached` is the in-memory mirror; its DURABLE copy is this tab's
  // session-scoped storage behind `link` (ADR-0051 §8), which carries a detach
  // across an F5 and kills it with the tab. Every transition goes through
  // `commitDetached` so the two never disagree.
  let detached: string[] = [];
  const fencePopups = new Map<string, PopupEntry>(); // fenceId -> { handle, members, fence, poll, peer }

  function isDetached(id: string) {
    return detached.includes(id);
  }

  // The detached fence ids, as a copy: they change only in `commitDetached`.
  function detachedIds(): string[] {
    return detached.slice();
  }

  // The popup entry of a fence, or `undefined`.
  function entry(id: string): PopupEntry | undefined {
    return fencePopups.get(id);
  }

  function has(id: string) {
    return fencePopups.has(id);
  }

  function size() {
    return fencePopups.size;
  }

  // Every [fence id, entry] pair, as a copy: a caller may remove an entry
  // while it walks the list (`reattachFence` from the heartbeat).
  function entries(): [string, PopupEntry][] {
    return [...fencePopups];
  }

  // Adds or replaces a fence's popup entry. The ids do not change here: the
  // caller commits them, after the entry, so `detachedMembers` can read it.
  function put(id: string, popup: PopupEntry) {
    fencePopups.set(id, popup);
  }

  function remove(id: string) {
    fencePopups.delete(id);
  }

  // A popup entry with no popup behind it yet — the shape both the boot restore
  // and an unheralded `popup-here` start from.
  function newPopupEntry(memberIds?: string[]): PopupEntry {
    return {
      handle: null,
      members: [],
      memberIds: memberIds || [],
      fence: null,
      poll: null,
      greeted: false,
      rescue: null,
      // Whether the POPUP has told us its member set. Until it has, an empty
      // `members` means "not asked yet" and the restored ids stand in; after it
      // has, an empty one means EMPTY — the operator closed them all in there.
      adopted: false,
      peer: { seen: Date.now(), lost: false },
      // Whether a silent window has already been probed: one unanswered probe
      // is death, not one quiet window (`stillThere`).
      probed: false,
    };
  }

  // The member ids each detached fence holds, for the registry: the live
  // snapshot, else the ids restored from the last write.
  function detachedMembers() {
    const out: Record<string, string[]> = {};
    for (const id of detached) {
      const entry = fencePopups.get(id);
      const live = (entry?.members || []).map((m) => m.id).filter(Boolean);
      // `adopted` is what lets an EMPTY live list mean empty: the popup answered
      // and holds nothing. Without it the fallback below would re-persist the
      // very consoles the operator just closed in there.
      out[id] = entry?.adopted || live.length ? live : (entry?.memberIds || []).slice();
    }
    return out;
  }

  // THE ONE PLACE `detached` CHANGES. INVARIANT on every return path: the
  // mirror and the stored registry hold the same ids, and no heartbeat timer
  // runs while nothing is detached.
  function commitDetached(next: string[]) {
    detached = next;
    link.writeRegistry(detached, detachedMembers());
    if (detached.length) startBeat();
    else stopBeat();
  }

  return {
    isDetached,
    detachedIds,
    entry,
    has,
    size,
    entries,
    put,
    remove,
    newPopupEntry,
    detachedMembers,
    commitDetached,
  };
}

export type PopupRegistry = ReturnType<typeof createPopupRegistry>;
