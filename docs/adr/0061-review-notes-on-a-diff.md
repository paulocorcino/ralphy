# A line note on a diff becomes one marked comment on the issue; pasting it into a live console waits for the agent to be at its prompt

Status: **proposed** (2026-09-15) — decided, not yet implemented. Fourth in
the track opened by [ADR-0058](./0058-checkout-per-run.md). Target A below
stands alone; Target B is gated on [ADR-0059](./0059-agent-state-by-hooks.md).

_Extends [ADR-0036](./0036-workbench-daemon-integration-protocol.md) §2 with
one Mutate verb and [ADR-0032](./0032-daemon-mode-supervised-launcher.md)
§6 with a second forge write (after `label.set`). Uses the marked-comment
primitive of [ADR-0017](./0017-agent-triage-entry-path.md). Leaves the
read-only diff of #311 intact. Adds one sentence to the plan charter._

## What a reviewer can do today, and what they cannot

The workbench shows a run's diff: HEAD via `blob.read`, the working side via
`file.read`, mounted in a Monaco diff with Find and nothing else —
"read-only by design (#311)" (`crates/ralphy-daemon/assets/ui/wb-viewer.js:158-162`).
An operator who sees a wrong line has two ways to say so, both outside the
workbench: open the issue on GitHub and type a comment the next planner
will read as discussion, or type into the console and hope the agent is
listening.

Ralphy's own loop already reads what a reviewer writes, in two places: the
issue's comments arrive in `issue.json` before planning (`types.rs:17-24`),
and a closed blocker's `## Handoff` comment is folded into `handoffs.md`
(`runner/artifacts.rs:107-136`). What is missing is a way to write a note
*at the line* and have it land where those readers look, without leaving
the diff.

The daemon may not write the repo or the forge itself: ADR-0036 §2 makes
every write "a new `ralphy` subcommand (never the daemon shelling out to
`git`/`gh`)", run-lock-aware. `label.set` already crossed from the forge's
read-only family into Mutate that way (ADR-0032 §6 said the family was
read-only "because nothing in the vocabulary writes" — that sentence is
amended below, not contradicted).

## Decision

### 1. One note format, deterministic, no preamble

A note is a file path, an optional line range, and a body. It renders as:

```
File: src/app.ts
Line: 10
User comment: "Needs validation"
```

with `Lines: 3-7` for a range and `Scope: file` for a note with no line;
the body is quoted with `\`, `"`, `\r`, `\n` escaped; several notes are
joined by one blank line. There is no instruction header: the reader is a
planner charter or a live agent, and both already know what a review note
is. The format is pinned by a test in `wb-viewer` (node) and by the Rust
side that receives it, so the two cannot drift.

### 2. Target A — the issue: one marked comment, upserted

Notes on a run's diff are sent to **the issue that run is working**:
`phase.active` (or `plan.issue`) from the run snapshot when the diff is a
run's; otherwise the operator picks an issue from `board.list`. They land
as one comment:

```
<!-- ralphy:review-notes -->
## Review notes

File: src/app.ts
Line: 10
User comment: "Needs validation"
```

via `IssueTracker::upsert_marked_comment` (`crates/ralphy-core/src/tracker.rs:67`),
so a second send **replaces** the comment rather than stacking one per
click — the same idempotence ADR-0017 gave `ralphy:consolidated-spec`.
The marker joins the inventory (`consolidated-spec`, `promote-evidence`,
`review-notes`).

Wiring, per ADR-0036 §2: a Mutate verb `issue.notes` in `dispatch.rs`,
composed as `ralphy issue notes <n> --file <path>`; a new CLI subcommand in
`mutate.rs` that guards the run lock like `label set` and calls the tracker.
The body travels as a file the daemon writes under its own directory and
names in argv (the `image.write` / `--file` pattern), never as shell text.

The **plan charter** (`assets/prompts/plan/template.md`) gains one
sentence in the artifact-reading section: *"A comment marked
`ralphy:review-notes` is the operator's line-by-line review of the previous
attempt; each note is a fact about the code it names, to be addressed or
explicitly declined in `## Decisions`."* No adapter changes: every planner
already receives the issue's comments.

### 3. Target B — a live console: paste only when the agent is at its prompt

The same rendered text can be pasted into a console over `/ws/session`,
which already carries `Frame::Terminal` bytes. It is sent as a bracketed
paste (`ESC[200~ … ESC[201~`) so the vendor treats it as one input, in
**two frames**: the text, then — after the daemon confirms the first frame
was written — a lone `\r`. Between the two the daemon re-checks the
session's `agent_state` (ADR-0059 §5); if it is no longer `done`/`waiting`,
the second frame is withheld and the UI says so. A paste is offered by the
UI only when the state is `done` or `waiting`; a session with no state
(vendor without hooks) gets no paste button, only Target A.

This target does not exist until ADR-0059 lands. It is written here so the
note format and the UI are designed for both from the start.

### 4. Notes live in the desk document until sent

Unsent notes are part of the operator's desk (ADR-0050: layout is daemon
state, `PUT /api/desk`), keyed by repo, file and line range, so a reload or
another browser sees them. They are cleared on a successful send to either
target. They are never written into the repo: a note is about the code, not
in it.

### 5. `blob.read` takes a revision

`blob.read` is HEAD-only (`dispatch.rs:191-193`). It gains an optional
`revision` argument, restricted to a ref name or `head` (validated by the
`ralphy blob read --revision` subcommand it already composes), so a run
branch's diff can be shown against its base — `branch.<b>.base` from
ADR-0058 §2 — and a note can say "since the base" rather than "since the
last commit".

### 6. What does not change

- The diff is still read-only for file content; #311 stands.
- The daemon runs no `gh`; the forge write is a `ralphy` subcommand.
- `handoffs.md` semantics: review notes are not handoffs. A handoff is what
  the executor tells the next issue; a review note is what the operator
  tells the next attempt at *this* issue. They reach the planner by
  different paths (the issue's own comments vs. `handoffs.md`) and the
  charter names them separately.

## Considered options — rejected

- **Write notes to a `.ralphy/review-notes.md` the planner reads.** Adds a
  file to every adapter's context inliner (Gemini enumerates named files)
  and a prompt slot per vendor; the issue comment is already read by all
  seven and is visible to the human on GitHub too.
- **One comment per note.** Ten clicks, ten comments, and the planner reads
  ten discussion entries. Upsert.
- **An instruction preamble in the note text** ("Please address the
  following…"). The charter is where instructions live; the note is data.
- **Paste into the console unconditionally.** Typing into an agent mid-turn
  interleaves with its own output and can answer a permission prompt by
  accident. The gate is the whole point of ADR-0059 for this feature.
- **A single frame with text and Enter.** If the agent's state flips
  between the check and the write, half the paste has already gone. Two
  frames, re-check between.
- **Store notes in `localStorage`.** ADR-0050 moved layout to the daemon
  precisely so a second browser sees the same desk; notes are desk state.
- **Make the diff editable while at it.** #311 decided the diff is a
  reading surface; editing belongs to the editor tab.

## Consequences

- An operator reviews a run's diff in the workbench and sends line notes to
  the issue without leaving it; the next plan for that issue starts from
  them. With ADR-0059, the same notes can go to a console the moment its
  agent is at the prompt.
- One Mutate verb (`issue.notes`), one CLI subcommand (`issue notes`), one
  marker, one plan-charter sentence, one `blob.read` argument, one desk
  field, and a Monaco glyph-margin affordance in `wb-viewer`.
- ADR-0036 §2's verb table, ADR-0032 §6 ("the forge family is read-only"
  → "reads plus two run-lock-aware writes, `label.set` and `issue.notes`"),
  ADR-0017's marker list and ADR-0050's desk schema each carry a one-line
  amendment. `docs/adr/0057` asset pins: the new node test must be imported
  by `ui-tests/index.mjs`.
- Changelog: `feature` — "leave line notes on a run's diff and send them to
  the issue as one review comment the next plan reads".

## Implementation notes (not decisions)

(1) `ralphy issue notes` + tracker call + marker, unit-tested against a fake
tracker; (2) `issue.notes` Mutate verb + daemon file handoff; (3) plan
charter sentence + prompt-assembly regen; (4) `blob.read --revision`;
(5) `wb-viewer` note UI + format test + desk field; (6) Target B after
ADR-0059 (5). (1)–(4) are green without the UI.
