# The scroll probe exists only in a developer build

Status: proposed
Kind: structural
Protects: responsiveness; constraint security

## Context

The operator finds the scroll of a console not smooth on an iPhone: it
stops and starts. Nobody can say why without numbers. On the iPhone the
console always draws with the DOM renderer (WebGL is off on WebKit,
`terminalGpu` in `wb-console-gpu.ts`), the page moves the lines itself on
each touch (`wb-console-touch.ts`), and the agent can write to the console
while it scrolls. Each of these can take the time of a frame.

Safari on an iPhone has no `longtask` entries and no developer tools
without a Mac. The page itself can still measure: `requestAnimationFrame`
gives the time of each frame, and `performance.now()` gives the time of
each piece of our own code.

The operator wants these numbers from the operator's own daemon only. A release must not
measure, send or store anything of this kind.

## Decision

**D1. The scroll probe is built only with the Cargo feature `scroll-probe`.**
The feature is in `ralphy-daemon`, and `ralphy-cli` forwards it. No workflow
under `.github/workflows/` turns it on, so no release candidate, release or
CI build has it. `scripts/update-ralphy.ps1 -ScrollProbe` builds the local
daemon with it.

**D2. The page sends a scroll record as the verb `probe.scroll` over
`POST /api/command`.** It is one more name on the one command route
(ADR-0082 D1), not a new route and not a new socket. The verb is served only
in a build with the feature, and it is not in the verb registry
(`dispatch::Verb`): a daemon without the feature answers `unknown verb`, as
for any name it does not know.

**D3. The page asks once per load.** After the page loads, it sends
`probe.scroll` with no records. When the reply is `unknown verb`, the page
does not measure for the rest of that load. The TypeScript code of the probe
ships in every build, but in a build without the feature it does nothing
after that first reply.

**D4. One record per scroll, a fixed shape.** A scroll starts with a touch
or a wheel on a console, and it ends when the console has not moved for
300 ms, after any glide. The record has only numbers and closed values:
how the scroll was made (touch, glide, wheel), its length in ms, the
number of frames, the frames over 25 ms, the median, p95 and longest
frame, the time of our touch code, the time of the terminal's writes and
the bytes it wrote, the renderer (WebGL or DOM), the rows and columns, the
lines of history, where the console was (stage, maximized, column, tab, full
screen) and the device pixel ratio. The page sends the records of a scroll
after it ends, never during it, and at most 20 records in one command.

**D5. The daemon writes one JSON line per record to
`~/.ralphy/probe/scroll.jsonl`.** The daemon adds the time and the **device**
of the request (ADR-0074). The file has owner-only permissions. Past 5 MiB
it is renamed to `scroll.1.jsonl`, and the older one is deleted. A record
that does not have the shape of D4 is refused and not written.

**D6. What this decision forbids.** No text of a console is ever in a
record: only counts and times. Nothing in the workbench shows the records,
and no setting turns the probe on or off: the build decides. No access
decision reads a record. The probe never changes how a console scrolls or
draws.

## Consequences

A developer reads the numbers: the file is read on
the machine, not in the workbench. A daemon updated with **Update now**
(the release) has no probe, and the page then stops measuring by itself.

The probe adds a Cargo feature, the first in the daemon. A change under the
feature is not built by CI, so its own tests run only in the local gate
with `--features scroll-probe`.

## Considered options

- **A Settings switch in every build.** Rejected by the operator: a
  release has no probe at all.
- **A local branch that never reaches `main`.** Every change to `main`
  would need a rebase, and the gate would not hold the code.
- **The audit log (ADR-0074).** The audit log records who did what. A scroll
  record is a measure, has its own size, and is deleted with no loss.

## Compliance

- D1: checked by `no_workflow_turns_on_the_scroll_probe`
  (`crates/ralphy-daemon/src/probe.rs`).
- D2: checked by `a_build_without_the_probe_answers_unknown_verb`
  (`crates/ralphy-daemon/src/routes/ws_command/tests.rs`).
- D3: not checked by code: the page side is reviewed in the PR.
- D4: checked by `a_record_out_of_shape_is_refused`
  (`crates/ralphy-daemon/src/probe.rs`).
- D5: not checked by code: the size limit and the permissions are reviewed in
  the PR.
- D6: not checked by code: reviewed in the PR.
