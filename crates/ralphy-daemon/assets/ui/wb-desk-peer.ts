/* ---------------------------------------------------------------------------
   A peer's liveness as a rule: (state, event, windowMs) -> { state, effects }.
   Pure, so it decides both directions of the link (origin watching popup,
   popup watching origin) and the node table drives the boundary.
   `createDetach` (`wb-desk-detach.ts`) returns `peerFold` to the console.
   --------------------------------------------------------------------------- */

// A popup peer's liveness: when it was last heard, and whether it is gone.
export type PeerState = { seen: number | null; lost: boolean };
export type PeerEvent = { type: "beat" | "tick" | "gone"; at?: number };
type PeerOut = { state: PeerState; effects: { type: "peer-lost" }[] };

// `windowMs` is an ARGUMENT, never a global.
//
// Loss is TERMINAL: a beat after `lost` does not resurrect — the caller turns
// the effect into a `window.close()` or a `reattachFence`, neither undoable.
// Loss is STRICT (`> windowMs`), so the boundary tick is still alive.
export function peerFold(
  state: PeerState | null | undefined,
  event: PeerEvent,
  windowMs: number,
): PeerOut {
  const seen = typeof state?.seen === "number" ? state.seen : null;
  const lost = !!state?.lost;
  const same = { seen, lost };
  if (lost) return { state: same, effects: [] };
  switch (event?.type) {
    case "beat":
      return {
        state: { seen: typeof event.at === "number" ? event.at : seen, lost: false },
        effects: [],
      };
    case "tick":
      return tickFold(same, event.at, windowMs);
    case "gone":
      // An announced departure: the same effect, without waiting the window out.
      return { state: { seen, lost: true }, effects: [{ type: "peer-lost" }] };
    default:
      return { state: same, effects: [] };
  }
}

// A tick of the peer's clock at `at`: the peer is lost once it has been
// silent for more than `windowMs`. `seen: null` — never heard from — expires
// nothing: the origin seeds every entry with `Date.now()` at creation AND at
// boot-restore, so one rule governs the post-reload adoption grace and steady
// state alike.
function tickFold(same: PeerState, at: number | undefined, windowMs: number): PeerOut {
  if (same.seen == null || typeof at !== "number") return { state: same, effects: [] };
  if (at - same.seen > windowMs) {
    return { state: { seen: same.seen, lost: true }, effects: [{ type: "peer-lost" }] };
  }
  return { state: same, effects: [] };
}
