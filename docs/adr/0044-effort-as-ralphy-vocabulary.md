# Effort becomes a Ralphy word: a five-rung ladder, normalized per adapter by failure mode

Effort is promoted from an opaque per-vendor passthrough to a first-class Ralphy
concept. Today `--exec-effort high` means five different things across the
adapters and nothing at all in three of them; after this ADR it means **one
word** — a rung on a fixed ladder — whose per-vendor effect is documented and,
where the vendor degrades silently, normalized so the operator can never ask for
more and quietly get less.

This closes the scope boundary ADR-0041 D5a deliberately drew: D5a clamped effort
*inside* the Copilot adapter and refused to make it a vocabulary, because "a
normalised vocabulary honoured by one vendor out of five is worse than none."
This ADR does the promotion across all **seven** adapters.

Grounded in live cross-vendor probes on 2026-07-22
([effort-vocabulary-probes.md](../research/effort-vocabulary-probes.md)); the
research doc records the observations, this ADR the decisions. Amends ADR-0004
D3, ADR-0005 D3, and ADR-0041 D5a. Consistent with ADR-0002 (core/adapter
boundary) and ADR-0039 (event vocabulary owned by `emit`).

Status: **proposed** — decisions settled from the design session and the probes;
implementation across the adapters is a follow-up (see *Wiring* and the residual
probes).

---

## The problem, in one table

| Adapter | Effort lives in | Real vocabulary | Out-of-range → |
|---|---|---|---|
| Claude | `--effort` | `low, medium, high, xhigh, max` | ⚠ warning + default, exit 0 |
| Codex | `-c model_reasoning_effort=` | 7 (OpenAI `reasoning.effort`) | ✅ API 400, loud |
| Copilot | `--effort`, per-model | 7, per-model catalog | ⚠ silent model default (P6) |
| Cursor | inside the model id | `none,low,medium,high,xhigh,max` | ✅ invalid-id error |
| Gemini | numeric `thinkingBudget` in settings | not a level | n/a |
| OpenCode | `--variant` | provider-specific, non-portable | provider rejects |
| Kimi | — | none | n/a |

Two facts drive every decision below: the **cross-vendor intersection is five
rungs**, and the vendors split into **silent degraders** (Claude, Copilot) and
**loud rejecters** (Codex, Cursor). Full evidence in the research doc.

## D1 — Effort is a Ralphy term, distinct from complexity routing and from model

Effort enters the ubiquitous language (CONTEXT.md): *a deterministic reasoning-
depth knob the operator sets per phase.* It is **not** complexity routing (the
planner auto-judging an issue and picking a model/tier) and **not** model
selection. CONTEXT.md already contrasts the three in prose; this ADR gives effort
its own entry so the word has one owner.

## D2 — The lexicon is five rungs: `low | medium | high | xhigh | max`

The probes show the exact intersection of the four vendors with a level axis
(Claude, Codex, Copilot, Cursor) is `low, medium, high, xhigh, max`. Claude is
the binding constraint — it publishes neither `none` nor `minimal` — so those two
rungs are **excluded from the core lexicon**: a word only two vendors honour is
the very "means something in one place" trap D5a warned against.

- `low | medium | high` are the **guaranteed universal** core — supported by
  every level-vendor and by every effort-capable model within them (proven by
  Copilot's `every_effort_model_supports_low_medium_high`).
- `xhigh | max` are in the cross-vendor intersection but **not** universal
  per-model within a vendor. They are valid inputs; the *guarantee* stops at
  `high`. A model that cannot honour them degrades per D4, never surprising the
  operator with more cost than asked.

This **supersedes** the design-session's earlier three-rung proposal: the
evidence widened the honest ladder from three to five. Effort and the
complexity-routing tier stay **separate scales** — the tier is three auto-judged
buckets, effort is five operator-set rungs. Forcing them onto one scale would
under-serve effort to buy a symmetry the vendors do not have.

Rejected: making effort a free string (today's shape) — it is what lets Claude's
and Copilot's silent degrade reach the operator. Rejected: the full seven-rung
set — `none`/`minimal` are not portable and `none` ("do not reason") is an edge
Claude cannot express at all.

## D3 — The lexicon is validated at the Ralphy boundary

`--plan-effort` / `--exec-effort` and the persisted `*_effort` settings are
validated against the five-rung enum at the CLI/config boundary, and a typo is
**refused at the keyboard**. This is not gold-plating: the probes show Claude
*silently ignores* an unknown value and runs at its default (exit 0), so an
unvalidated typo is indistinguishable from a working setting until a run quietly
under-delivers. Copilot already validates its persisted key this way
(`is_known_effort`); this generalizes that guard to the core word.

## D4 — Translation lives in the adapter, chosen by the vendor's failure mode

Core owns the **word**; each adapter owns the **dialect** (the ADR-0002 seam).
There is **no normalization engine in core** — the per-model support table that
makes clamping safe exists for exactly one vendor (Copilot's catalog), and
lifting it to core would force six adapters to reason about rungs they cannot
validate. Instead each adapter declares one of four postures:

| Posture | Adapters | Behaviour |
|---|---|---|
| **Clamp** (silent vendor, has a catalog) | Copilot | Clamp the request down to the model's greatest supported level; omit if none. **Mandatory** — the vendor hides the drop. |
| **Passthrough** (loud vendor) | Codex, Cursor | Forward the validated word; if the model rejects it, the vendor errors loudly. No clamp needed. |
| **Direct map** (fixed scale, no per-model table) | Claude | Forward the word; it lands on the CLI's own five-rung enum. Claude publishes **no per-model catalog** (probed), so a clamp is not buildable and Direct map is the ceiling by necessity — the boundary validation (D3) is the only guard. |
| **No-op** (no level axis) | Kimi, Gemini, OpenCode | Accept the word, **document that it does nothing here**, emit it as absent. |

The rule that assigns a posture is mechanical: *silent + catalog → clamp; loud →
passthrough; no axis → no-op.* No per-adapter bespoke normalization beyond the
existing translate-to-argv point each adapter already has.

## D5 — The neutral word flows into all seven adapters, not just Claude

Today `--plan-effort`/`--exec-effort` reach **only** the Claude adapter; every
other adapter takes effort from its own settings, a constant, or `""`. This ADR
threads the resolved word into every adapter's `plan()`/`execute()` — a
one-site change per adapter in `run.rs`/`wiring.rs`, no new mechanism. The three
adapters with no axis keep their `let _ = effort;` no-op, but it becomes a
**first-class, documented** no-op that emits `effort = None` rather than a
misleading value.

## D6 — Copilot's clamp stays in-crate; the seven-rung ordering never moves to core

Core knows only the five-rung enum. Copilot's `EFFORT_ORDER` (the seven-rung
superset) and `clamp_effort` **stay inside `ralphy-agent-copilot`**, and the
guard `clamp_lives_only_in_the_copilot_adapter` **stays green** — the core word
flows *into* the existing `resolve_effort`, it does not pull the ordering out.
Copilot additionally continues to accept `none`/`minimal`/`max` through its
persisted `copilot.*_effort` key for operators on capable models; those are
Copilot-local extensions above the core lexicon, clamped as always. The five-rung
core and the seven-rung Copilot superset coexist with zero leak.

## D7 — Codex honours operator effort; the default stays `medium`

ADR-0004's amendment already declared the intent — effort as "a single global
operator override (opt-up to `high`/`xhigh`)" — it was simply never wired. This
ADR wires it: the resolved word sets `model_reasoning_effort`, defaulting to
`medium` when unset. Effort and the tier→model routing stay **orthogonal**: the
tier picks the model (sol/terra/luna), effort picks how hard that model thinks.
The probe confirms Codex accepts the full set and errors loudly on a bad one, so
no clamp is needed. This amends ADR-0004 D3's frozen-effort clause.

## D8 — OpenCode keeps `--variant` as its raw provider knob; the neutral word is a no-op there

OpenCode's `--variant` vocabulary is provider-specific and non-portable
(ADR-0005 D3), and OpenCode has **no catalog** to clamp against. Mapping the
neutral word onto `--variant` blindly would re-introduce the exact silent-reject
this ADR exists to kill. So: `--exec-variant` remains the operator's provider-
native escape hatch, and the neutral effort word is a **documented no-op** for
OpenCode (the *No-op* posture). This amends ADR-0005 D3 only in vocabulary —
`--variant` is OpenCode's *dialect*, not Ralphy's *effort* — and the telemetry
split in D9 follows from it.

## D9 — Events carry the neutral word or `None`; variant is no longer folded into effort

`emit::planning`/`executing` and the CloudEvents `data.agent.effort` field
already exist (ADR-0039). This ADR fixes what feeds them:

- Level-vendors emit the resolved neutral rung.
- No-op vendors emit `None` (an empty string already folds to `None`).
- **OpenCode stops folding `--variant` into the effort slot** — a variant is a
  model-variant selector, not effort. It is reported as `variant`, and `effort`
  is `None`. `runstate/fields.rs`'s current `"effort" | "variant" => effort`
  fold is the misreport this corrects.

## What this ADR deliberately does not decide

- **Cursor via the model id.** Cursor's effort lives inside the model-id string,
  and the probe (WSL, live) *confirmed* both forms — the `-high`/`-xhigh` suffix
  (via `--list-models`) and the `[effort=high]` bracket (help-documented) — and
  found that Cursor publishes a **free per-model effort catalog**, the Copilot
  shape. So the grammar is settled; what is deferred is only the *wiring* choice:
  Cursor moves from *No-op* to encoding the resolved rung into the id, either as a
  loud *Passthrough* (an unsupported level yields an id the vendor rejects) or a
  *Clamp* against `--list-models`. A capability gain, not a correctness gap — and
  no longer blocked on a probe.
- **Gemini's numeric budget.** Mapping the five rungs onto `thinkingBudget`
  numbers is a design decision with no empirical answer to probe; Gemini stays
  *No-op* until the mapping is designed.
- **Claude per-model clamp — probed and bounded.** Claude accepts a valid level
  on any model with no per-model signal and publishes no catalog, so a clamp is
  not buildable. Direct map with boundary validation (D3) is the ceiling; a silent
  per-model degrade, if it exists, is unobservable and uncorrectable — the same
  no-catalog class as OpenCode, but with the value enum still validated.

## Wiring (follow-up, not part of accepting this ADR)

Per the ADR-0040 spirit, the edit sites are: the five-rung enum + boundary
validation in `ralphy-cli` (`cli.rs`, `config.rs`/`run.rs`); the resolved word
threaded into every adapter in `run/wiring.rs`; per-adapter posture
(`ralphy-agent-*`); the telemetry split in `runstate/fields.rs`; the CONTEXT.md
`Effort` entry (D1); and this ADR's amendments to 0004/0005/0041. Copilot needs
**no** change to its clamp (D6). Kimi/Gemini keep their no-op, now documented.
