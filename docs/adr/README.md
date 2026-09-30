# Architecture Decision Records

Each ADR records one decision about a seam in Ralphy. Before changing a seam,
check for the ADR that already governs it — the boundary you are about to cross
was probably decided on purpose (see [CLAUDE.md](../../CLAUDE.md)).

## Numbering convention

- **One number, one decision.** A file `NNNN-<slug>.md` is a distinct
  architecture decision. Numbers are allocated in order and never reused for an
  unrelated decision. Prose cites a decision by its number (`ADR-0002`), so a
  number must resolve to exactly one decision or the citation is ambiguous.

- **Companion notes share the parent number.** A vendor adapter's decision is
  followed by *validation* (and sometimes *revalidation*) notes recorded as
  `NNNN-<vendor>-validation.md` / `NNNN-<vendor>-revalidation.md`. These are
  **not new decisions** — they are the phased follow-up (live-validation
  findings, drift re-grounding) *under the same decision*. A reader tells them
  apart by the suffix: the bare `NNNN-<vendor>-adapter.md` is the decision; a
  `-validation` / `-revalidation` sibling is its companion note. Citing
  `ADR-NNNN` means the decision; a companion note is cited by its full filename
  when the distinction matters.

  Current companion-note families: **0005** (opencode), **0028** (kimi),
  **0041** (copilot), **0042** (cursor), **0043** (gemini), **0052** (local
  fleet).

## Writing a new ADR

Start from [TEMPLATE.md](./TEMPLATE.md). It says what to put in each field.
Two rules apply to every ADR from ADR-0068 on:

- **A status line from a closed set**, kept true to the code:
  `proposed | accepted | deferred | rejected | superseded by ADR-NNNN`.
- **A structural ADR has a `## Compliance` section.** A structural ADR decides
  a boundary, a dependency rule, a disk or wire format, or who owns a fact.
  Compliance gives one line per rule: the check that fails when code breaks
  it, or "not checked by code" and why.

Older ADRs are not rewritten. They get a Compliance section when they are next
amended.

## The 0002 ↔ 0004 core/adapter boundary

The **core/adapter boundary** — "core is execution-mode-agnostic; adapters own
how an agent is driven" — is **ADR-0002**
([`0002-core-agnostic-adapter-boundary.md`](./0002-core-agnostic-adapter-boundary.md)).
That is the number the prose cites and the file it resolves to.

Historically the number 0002 also hosted an unrelated decision (blocked-by
gating) and some prose miscited the boundary as "ADR-0004" (which is the *Codex
adapter*). Both were resolved in #293: blocked-by gating was renumbered to
[ADR-0045](./0045-blocked-by-gating.md), and the drifted "ADR-0004" citations
that meant the boundary were corrected to ADR-0002. When you see "ADR-0004" it
means the [Codex adapter](./0004-codex-adapter.md) and nothing else.
