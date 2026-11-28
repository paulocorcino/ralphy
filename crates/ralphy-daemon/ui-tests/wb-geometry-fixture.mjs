// The fences, rects and members that more than one wb-geometry test file reads.
export const FENCE_VIEW = { width: 1400, height: 900 };
export const ORIGIN = { left: 0, top: 0 };

// Two abutting fences. Membership is DERIVED from the middle of the title bar
// (x = centre, y = top + min(16, height / 2)), never stored: no window record
// gains a `fenceId`, so a fence and a window can never disagree about it.
// Containment is HALF-OPEN (`left <= x < left + width`) because fences may
// abut — a closed test would put a point sitting on the shared border inside
// BOTH of them, breaking "exactly one fence".
export const AB = [
  { id: "a", rect: { left: 0, top: 0, width: 100, height: 100 } },
  { id: "b", rect: { left: 100, top: 0, width: 100, height: 100 } },
];

export const FIT = { left: 100, top: 100, width: 100, height: 100 };

export const EXISTING = [{ id: "e", rect: FIT }];

export const members = (n) => Array.from({ length: n }, (_, i) => ({ id: `m${i}` }));
