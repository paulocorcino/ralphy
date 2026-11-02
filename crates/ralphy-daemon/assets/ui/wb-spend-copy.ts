// The words of the Spend tab: the states the pane can be in, and what each unpriced cause
// means. Pure data, as `wb-spend.ts`, which re-exports them in `WBSpend`.

// The four states the pane can be in, named so the markup branches on a word
// instead of on a combination of falsy fields.

export const EMPTY = "empty"; // no project open — the operator is told to open one
export const LOADING = "loading";
export const ERROR = "error";
export const READY = "ready";

// What each unpriced cause MEANS, keyed by the daemon's closed vocabulary. The
// split exists because one bucket shrinks with work and another never will
// (ADR-0053 D4) — so each row states which it is, rather than leaving three
// numbers to be told apart by name alone.
export const CAUSE_COPY: Record<string, { title: string; hint: string }> = {
  recoverable: {
    title: "recoverable",
    hint:
      "The session ID is recorded. " +
      "The model can still be read from the vendor's session store.",
  },
  no_price: {
    title: "no price",
    hint: "The model is not in the price table.",
  },
  lost: {
    title: "lost",
    hint: "No session ID is recorded. The model cannot be recovered.",
  },
  // A Ledger row only: an interactive record the vendor keeps no count for.
  unmetered: {
    title: "no token count",
    hint: "The vendor keeps no token count for this session.",
  },
};
