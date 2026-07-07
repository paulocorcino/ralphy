# A native scheduling command: `ralphy schedule`

Status: proposed.

`docs/scheduling.md` gives tested, copy-pasteable recipes for re-invoking
`ralphy run` on a timer (Windows Task Scheduler, cron, GitHub Actions), and its
closing note flags the obvious next step:

> A `ralphy schedule install|status|remove` subcommand that registers the timer
> natively is the planned next step; these recipes double as its per-platform
> specification.

That is [#72](https://github.com/paulocorcino/ralphy/issues/72)'s out-of-scope
item, promoted here to a decision. Ralphy shelling out to the operating
system's scheduler is a **new seam** — the process crossing from its own address
space into Task Scheduler / crontab registration — and per CLAUDE.md a seam gets
an ADR before its implementation slices, so the slices build against a fixed
surface rather than discovering it. This ADR fixes that surface; it does not
implement it.

The recipes already settle the *mechanics* per platform (working directory, auth
availability, log capture, the `--if-idle` overlap guard). What is undecided is
the **command shape** on top of them: where the subcommand hangs, what its
defaults register, how a triage→run chain is expressed, and what safety
invariant must hold before Ralphy is allowed to install a second recurring timer
at all. Those are the decisions below.

## Decision

### 1. A top-level `schedule` command, not a flag on `run`

Scheduling is its own top-level command:

```
ralphy schedule install <run|triage>
ralphy schedule status
ralphy schedule remove <run|triage|--all>
```

It is **not** a `--schedule` flag hanging off `run`. The deciding case is the
triage→run chain (§3): chaining triage before a run is cross-command
orchestration — it composes two commands into one recurring window — and
orchestration of commands does not belong to either command it orchestrates. A
flag on `run` could never own the triage half without `run` reaching across the
boundary ADR-0009 draws between the two entry paths. A dedicated command owns the
composition cleanly, and `status`/`remove` need a noun of their own regardless:
there is no natural home on `run` for "list the timers this repo has registered."

This keeps Ralphy *the run, not the cron* in spirit even while it gains a cron
installer: `schedule` **registers** an OS timer that later invokes `ralphy run` /
`ralphy triage`; it never becomes the scheduler itself. The timer lives in the
OS; Ralphy only writes and removes its registration.

### 2. Defaults are correct-by-construction per target

Each `install` target registers the invocation that is *right* for that target,
so the safe form is the default and the operator does not hand-assemble flags:

- **`install run`** registers `ralphy run --if-idle`. `--if-idle` is the
  scheduled-invocation overlap guard (#72 item 3): a firing that finds a live run
  logs `skipped: run in progress …` and exits 0, never piling a run on a run.
  Scheduling without it would be the wrong default the first time two firings
  overlap.
- **`install triage`** registers `ralphy triage --if-idle --yes`. `--yes` is not
  an add-on convenience here — *scheduling triage is itself the unattended-promotion
  trust act*. Under ADR-0017 the human's trust act is applying the `triage-agent`
  label; `--yes` then lets the scheduled session publish and promote directly as
  the mechanical continuation of that decision. An operator who registers a
  recurring triage timer has authorized exactly that continuation, so `--yes`
  belongs in the default. `--if-idle` carries the same overlap guarantee into
  triage (see §4 — the lock now covers triage too).

Defaults are correct-by-construction, not merely convenient: the wrong
invocation is not reachable through the common path. An operator who wants a
different shape edits the registered task afterward, exactly as with any recipe
in `docs/scheduling.md`; `schedule install` owns the *blessed* default, not the
full space of timer configurations.

### 3. `--with-triage`: a single-window chained timer on the `run` target

`ralphy schedule install run --with-triage` registers **one** timer whose action
is the chained pair, triage first:

```
ralphy triage --yes ; ralphy run --if-idle
```

This is the native-installer form of the two-phase recipe ADR-0017 §5 documents
and `docs/scheduling.md` spells out. One timer, one window, deterministic
ordering: *today's promotions land in today's queue*, because the triage half
promotes `triage-agent` issues to `ready-for-agent` before the run half builds
its queue in the same firing.

Two decisions are inherited verbatim from ADR-0017 and are load-bearing, not
cosmetic:

- **`;`, never `&&`.** A broken triage costs only the triage, never the night's
  execution of issues already `ready-for-agent`. The run half must fire whether
  or not the triage half succeeded.
- **Triage first.** The ordering is what makes "one window" mean anything — a run
  before triage would drain a queue that this firing's promotions never joined.

`--with-triage` lives on the **`run`** target, not as a third `install` noun,
because the artifact is a run timer that happens to warm its own queue first; a
bare `install triage` (a standalone promotion timer, no run) remains available
for operators who separate the two cadences.

### 4. Repo-scoped presence lock — the safety prerequisite

This is the invariant that must hold **before** `schedule` may exist, and stating
it is the point of doing the ADR first.

Today `.ralphy/run.lock` is **run-owned**: `crates/ralphy-cli/src/runlock.rs`
describes it as the presence lock "for `ralphy run`", and only `run` writes and
honors it (#72 item 3). That was sufficient while a run was the only thing a
schedule could fire. `schedule` breaks that assumption: it can register a triage
timer *and* a run timer against the same repo, and — absent a shared lock — a
triage firing and a run firing (or two triage firings) could execute
concurrently against one working tree. `ralphy triage` and `ralphy run` both
mutate `.ralphy/` and drive an agent against the same checkout; overlapping them
is precisely the hazard `--if-idle` was introduced to prevent for run×run.

So the lock is **promoted from run-owned to repo-owned**: `.ralphy/run.lock`
becomes a repo-scoped presence lock honored by **both** `run` and `triage`. One
lock, one repo, whichever command holds it. This closes all three overlap pairs
the new command makes reachable — triage×triage, triage×run, run×triage — with
the single mechanism that already works for run×run, rather than inventing a
second lock and a second liveness rule to keep in sync. The lock's existing
semantics are unchanged and reused wholesale: PID + start-time payload, signal
not mutex (manual runs never block each other), `--if-idle` defers-and-exits-0 on
a live lock, plain warning without the flag, stale-PID takeover so a crash never
silences a schedule permanently.

Concretely this means the lockfile's ownership doc moves from "for `ralphy run`"
to "for this repo," and `triage` acquires the lock on entry and honors `--if-idle`
against it exactly as `run` does — the implementation slice's job, fixed here as
the seam's precondition. `schedule` should not register a triage timer in a repo
whose `triage` cannot yet honor the repo-scoped lock; the lock promotion lands
first.

### 5. GitHub Actions stays a documented recipe, never a native `install` target

`schedule install` registers a **local OS timer** only — Windows Task Scheduler
or cron, the machine's own scheduler. GitHub Actions is deliberately **not** an
`install` target:

- The Actions recipe is a repo-committed workflow file (`.github/workflows/…`)
  under the operator's version control and review, with its own secrets, runner,
  and `concurrency` group — not a machine-local timer Ralphy writes and removes.
  Ralphy generating and committing a workflow would cross from "install a timer on
  this box" into "write to your CI configuration," a different and larger act.
- Its overlap story is different in kind: on ephemeral runners the PID-based
  `run.lock` can only see runs on that runner, so the `concurrency` group is the
  real guard (`docs/scheduling.md` documents this). A native `install` abstraction
  that pretended Actions and cron were the same target would paper over that
  difference.

`docs/scheduling.md` remains the **per-platform specification** for all three
recipes, and the authoritative source for the Actions path specifically.
`schedule install` implements the Task Scheduler and cron recipes natively and
points at the doc for Actions.

## Consequences

- The scheduling surface is fixed before implementation: subsequent slices build
  `install`/`status`/`remove` against the command shape, defaults, and lock
  precondition decided here, rather than re-deciding them per slice.
- **The repo-scoped lock promotion is the gating first slice.** Promoting
  `.ralphy/run.lock` from run-owned to repo-owned and teaching `triage` to
  acquire and honor it is the safety prerequisite; `schedule` (especially any
  triage or `--with-triage` target) must not ship ahead of it. The lock's
  semantics are reused unchanged — this is an ownership widening, not a new
  mechanism.
- Defaults are correct-by-construction: `install run` → `run --if-idle`,
  `install triage` → `triage --if-idle --yes`, `install run --with-triage` →
  `triage --yes ; run --if-idle`. The overlap guard and the triage trust act
  (ADR-0017) are baked into the blessed path, not left to the operator to
  reassemble.
- `ralphy schedule status` and `remove <run|triage|--all>` give the timers a
  noun `run` could never host — listing and removing the registrations this repo
  installed.
- Ralphy stays *the run, not the cron*: `schedule` registers OS timers and
  removes them; it never runs the loop itself. Non-users pay nothing.
- GitHub Actions remains a documented recipe, not a native target;
  `docs/scheduling.md` stays the per-platform spec and the sole authority for the
  Actions path. #72's out-of-scope note is hereby resolved into this decision.
- Cross-platform, per CLAUDE.md: `install` must speak both Task Scheduler
  (`schtasks`/`Register-ScheduledTask`) and cron/crontab from the same command,
  and its registration/removal logic is tested against both without a live
  scheduler — the recipes in `docs/scheduling.md` are the conformance target.
