# Read-time model recovery: an unpriceable ledger line is repaired by projection, never by rewrite

Status: accepted.

A ledger line whose `model` is `unknown` cannot be priced. This is **not** a
price-table gap that ADR-0034 slice A's models.dev fetch closes — the line never
recorded *which* engine spent the tokens, so there is no key to look up. This ADR
decides how such a line is repaired: by a **read-time projection** that joins the
line's `session_id` to the vendor session store the **usage scan** (ADR-0033)
already reads, with the resolved pairs kept in a **persisted, append-only map**
beside the ledger. The ledger itself is never rewritten. Vocabulary
(**unpriced volume**, **model recovery**, **retry burn**) lives in
[CONTEXT.md](../../CONTEXT.md).

## Why now

Measured against the operator's own ledger (`~/.ralphy/usage/*.jsonl`, 626 lines,
3.78 B tokens, 2026-06-15 → 2026-07-27) while designing the workbench spend
surface:

```
tokens with model == "unknown"      1663.2M   44.0% of all tokens
├─ carrying a session_id             737.3M   44.3% of those  → recoverable
└─ no session_id at all              925.8M   55.7% of those  → unrecoverable

by agent (unknown tokens · recoverable share)
  claude    1166.2M   29.8%      codex    341.9M  100.0%
  kimi       109.5M    2.4%      opencode  44.3M  100.0%
  cursor       0.7M  100.0%      gemini     0.6M  100.0%      copilot 0.0M  0.0%
```

Two facts make this urgent rather than merely desirable. First, **the recent
gap is fully recoverable**: Codex alone accounts for 341.9 M unknown tokens —
86.5 % of its recent volume — and 100 % of them carry a `session_id`. Second,
**the window is closing**: recovery reads vendor session stores, which are
pruned. Every store cleanup converts recoverable tokens into permanently lost
ones. The cost surface that motivated the measurement (a per-project spend view
with cost-per-delivery) would otherwise open reporting a floor ~20 % below
truth, which reads as a broken dashboard rather than as an instrumentation gap.

Recovery is **remediation of the past**. The Codex adapter writing `unknown`
*today* is a separate, prospective bug fix; neither substitutes for the other.

## D1 — Feasibility is proven, not assumed

The join was executed against real data before this ADR was written: all **32**
Claude ledger lines with `model == "unknown"` and a `session_id` were located in
`~/.claude/projects` (1300 sessions on disk) and **32 of 32** yielded a model
(`claude-opus-4-8`, `claude-sonnet-5`, …). The mechanism is not speculative.

No new reader is written. `ralphy-usage-scan` already parses every vendor's
session store and already extracts the model — that is its purpose (ADR-0033).
Recovery reuses those readers; it adds a join, not a capability.

## D2 — The repair is a projection; the ledger is never rewritten

Three shapes were considered:

1. **Rewrite the lines in place.** Direct, and it destroys the append-only
   property that makes the ledger auditable (ADR-0008 D6). A defect in the
   reconstructor corrupts history irreversibly, and the ledger is written
   best-effort by contract — it has no transaction to roll back.
2. **Append correction lines.** Append-only survives, but every reader must
   learn a correction fold, and a fold defect double-counts *tokens* — corrupting
   the one thing ADR-0008 D2 calls the unit of truth.
3. **A side map, applied at read time.** The ledger is untouched. A reader that
   meets `model == "unknown"` consults the map.

**(3) is chosen**, because it is the shape this codebase has already chosen for
the same class of problem: USD is never stored and is projected at read time over
an immutable ledger (ADR-0008 D2, ADR-0034 D3). A recovered model is the same
species of derivation — the ledger records what happened, the reader enriches it.
It is reversible, it is testable against a fixture without a filesystem write to
the ledger, and a wrong recovery destroys nothing.

## D3 — The map is persisted, never recomputed

Deriving the map on demand would be simpler and is **wrong**: vendor stores are
pruned, so an on-demand join silently returns less every month while appearing to
work. A pair resolved today is a permanent fact; the ability to resolve it is
not. The map is therefore a durable artifact next to the ledger — `session_id →
model`, **append-only, resolved once, kept forever** — and running the resolver
early has standalone value independent of any surface that consumes it.

The map is a **cache of a fact, not a source of truth**: losing it costs
recoverable history (bounded by what the stores still hold), never tokens.

## D4 — Unrecoverable is reported as *lost*, not as *pending*

A line with no `session_id` has no key to any store — 55.7 % of unknown tokens
today. The surface distinguishes **recovered**, **recoverable**, and **lost**,
because the first two shrink with work and the third never does. Reporting all of
it as one undifferentiated gap invites indefinite re-investigation of tokens that
are gone.

This upholds ADR-0034 D3 unchanged: an unpriceable line contributes `~$?`, never
`$0`, and a total carrying any of them is a **floor**, marked as such.

## Consequences

- A new durable artifact under the usage root, `session_id → model`, append-only.
  Neither the ledger's format nor its append-only contract changes; no existing
  reader breaks, and a reader that ignores the map still produces today's answer.
- `ralphy-usage-scan` gains a resolver built from its existing per-vendor
  readers. It gains no new vendor knowledge.
- ~44 % of historically unknown tokens become priceable; the residue is
  permanent and is labelled as such. The all-time unpriced share falls from
  ~44 % to ~24 %, and to near zero over the recent window.
- Deliberately **not** decided here: the Codex adapter's live `unknown` defect
  (a prospective fix, separately tracked), adding token counters to the run
  snapshot for live in-flight spend (new instrumentation, out of scope), and the
  spend surface's own layout (reversible, no ADR).
