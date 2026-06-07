# Triage vocabulary follows Matt Pocock's canonical roles; flow control is separate

Ralphy adopts [Matt Pocock's canonical triage roles](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills).
`ready-for-agent` (alias `AFK`) is the queue; `ready-for-human` (alias `HITL`) is
human-only and **never** worked by the agent — it is simply never queried,
carrying no runtime behaviour.

We rejected overloading `ready-for-human` to mean "agent works it but leaves it
open for a human", because that contradicts the canonical mutual exclusivity of
the roles (an issue is `ready-for-agent` *or* `ready-for-human`, not both). We
also rejected a third hybrid label. The need to pause a run mid-sequence is met
instead by a separate flow-control label, `stop-before`: it halts the run
**before** its issue (issues earlier in the sequence still run), and the human
removes it and re-runs to continue. `stop-before` is a fixed, non-configurable
label so it stays out of the triage vocabulary.

A green queue issue is closed by the runner to complete the cycle — the label is
left untouched and the branch is still merged by hand. We accept that this closes
an issue whose code is not yet merged: closing signals "the agent finished its
part", and the run branch is the artifact the human reviews and merges.
