# Scheduling `ralphy run`

Ralphy is deliberately *the run, not the cron*: it owns one queue-draining run
from invocation to exit and leaves the timer to the operating system's
scheduler. This page is the scheduling floor above it — tested, copy-pasteable
recipes for re-invoking `ralphy run` on a timer with Windows Task Scheduler,
cron, and GitHub Actions.

Every recipe invokes **`ralphy run --if-idle`** as the primary anti-overlap
mechanism: each run holds a presence lock (`.ralphy/run.lock`, its PID and
start time inside) for its lifetime, and an `--if-idle` invocation that finds
the lock pointing at a living process logs
`skipped: run in progress since <time>, pid <X>` and exits **0** — a clean
no-op, so the scheduler's history shows no false failures and simply retries
next tick. A stale lock (dead PID after a crash or reboot) is ignored and taken
over, so a crash never silences the schedule permanently. Without `--if-idle`
a live lock only produces a warning and the run proceeds — intentional
concurrency stays the human's call.

Scheduler-native guards (Task Scheduler's instance policy, `flock -n` for
cron, Actions `concurrency` groups) are layered in each recipe as secondary
defense.

> **Verification status**: the Windows Task Scheduler recipe was verified
> end-to-end on Windows 11 (task registered, fired on schedule, one firing ran
> and one deferred to a live run with the `skipped:` line and exit 0). The
> cron and GitHub Actions recipes follow the same pattern and were
> review-verified only.

---

## Windows Task Scheduler

Register a task that runs the queue every 30 minutes as the logged-in user:

```powershell
$repo   = "C:\path\to\your\repo"
$log    = "$env:USERPROFILE\ralphy-schedule.log"
$action = New-ScheduledTaskAction -Execute "pwsh" `
  -Argument "-NoProfile -Command `"Set-Location '$repo'; ralphy run --if-idle *>> '$log'`""
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date) `
  -RepetitionInterval (New-TimeSpan -Minutes 30)
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew `
  -StartWhenAvailable
Register-ScheduledTask -TaskName "ralphy-run" -Action $action `
  -Trigger $trigger -Settings $settings
```

Or the `schtasks` one-liner equivalent:

```powershell
schtasks /Create /TN ralphy-run /SC MINUTE /MO 30 /TR `
  "pwsh -NoProfile -Command \"Set-Location 'C:\path\to\your\repo'; ralphy run --if-idle *>> '$env:USERPROFILE\ralphy-schedule.log'\""
```

### Traps

- **Working directory** — Task Scheduler starts actions in `%WINDIR%\System32`,
  not your repo. Always `Set-Location` (or `New-ScheduledTaskAction`'s
  `-WorkingDirectory`) before `ralphy run`, or ralphy resolves the wrong git
  toplevel and fails.
- **Auth in non-interactive sessions** — the task must run as the *same user*
  whose `%USERPROFILE%` holds the agent CLI's credentials (`~/.claude`) and
  `gh auth login` state. Never run it as SYSTEM. With "Run whether user is
  logged on or not", the task runs in a non-interactive session: a logged-out
  agent CLI cannot pop a browser to re-authenticate there — ralphy now fails
  fast with a clear `not authenticated` error instead of stalling into a
  timeout, and the fix is to log in once from a normal terminal.
- **Log capture** — Task Scheduler swallows stdout/stderr. The `pwsh -Command`
  wrapper with `*>> $log` above appends everything to a file; without it a
  failing run leaves no trace beyond the task's last-result code.
- **Avoiding overlapping runs** — `--if-idle` is the primary guard.
  `-MultipleInstances IgnoreNew` (the default policy for `schtasks`-created
  tasks) is the secondary, scheduler-native one: it stops Task Scheduler from
  even launching a second instance while the first is still running.

### Verify it worked

```powershell
Get-ScheduledTaskInfo -TaskName "ralphy-run"   # LastRunTime / LastTaskResult (0 = ok)
Get-Content "$env:USERPROFILE\ralphy-schedule.log" -Tail 20
```

Fire it once by hand with a run already live to see the deferral:

```powershell
Start-ScheduledTask -TaskName "ralphy-run"
# log shows: skipped: run in progress since <time>, pid <X>
```

Remove it with `Unregister-ScheduledTask -TaskName "ralphy-run" -Confirm:$false`.

---

## cron (Linux/macOS)

```cron
# m  h  dom mon dow  command
*/30 *  *   *   *    cd /path/to/your/repo && flock -n /tmp/ralphy-cron.lock ralphy run --if-idle >> "$HOME/ralphy-cron.log" 2>&1
```

If `ralphy` isn't on cron's minimal `PATH`, use absolute paths or set `PATH`
at the top of the crontab:

```cron
PATH=/usr/local/bin:/usr/bin:/bin:/home/you/.local/bin
MAILTO=""
```

### Traps

- **Working directory & PATH** — cron runs with a minimal environment (`PATH`
  is usually just `/usr/bin:/bin`) and no shell profile. `cd` into the repo
  explicitly and reference `ralphy`/the agent CLI by absolute path or via a
  `PATH=` line in the crontab.
- **Auth in non-interactive sessions** — the crontab must belong to the user
  whose `$HOME` holds the agent CLI credentials and `gh` auth. A logged-out
  CLI cannot re-authenticate under cron; ralphy fails fast with a clear
  `not authenticated` error — log in once from a normal shell.
- **Log capture** — cron discards output unless redirected (or mails it, which
  usually goes nowhere). Always `>> logfile 2>&1`; set `MAILTO=""` to silence
  the mailer.
- **Avoiding overlapping runs** — `--if-idle` is the primary guard. `flock -n`
  on a system-wide lockfile is the secondary, scheduler-native one: it refuses
  to even start the second command while the first holds the flock (note
  `flock` guards the *invocation*, ralphy's `run.lock` guards the *repo*).

### Verify it worked

```bash
tail -20 "$HOME/ralphy-cron.log"
grep -E "ralphy run|skipped: run in progress" "$HOME/ralphy-cron.log"
```

---

## GitHub Actions `schedule`

```yaml
name: ralphy-scheduled-run
on:
  schedule:
    - cron: "0 */2 * * *"   # every 2 hours (UTC), best-effort timing
  workflow_dispatch: {}      # manual trigger for testing

concurrency:
  group: ralphy-run          # scheduler-native guard: one run at a time
  cancel-in-progress: false  # queue, don't kill a live run

jobs:
  run:
    runs-on: ubuntu-latest
    permissions:
      contents: write        # the run pushes branches
      issues: write          # and works/labels issues
    steps:
      - uses: actions/checkout@v4
      - name: Install agent CLI + ralphy
        run: |
          npm install -g @anthropic-ai/claude-code
          # install ralphy (release binary or cargo install), e.g.:
          # cargo install --git https://github.com/paulocorcino/ralphy ralphy-cli
      - name: Run the queue
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: ralphy run --if-idle --headless-exec
      - name: Upload run logs
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: ralphy-logs
          path: .ralphy/runs/
```

### Traps

- **Ephemeral runners** — each firing starts on a fresh runner with a fresh
  checkout, so `.ralphy/run.lock`'s PID check can only see runs *on that
  runner*. The `concurrency` group is the real overlap guard here; `--if-idle`
  is kept for consistency (and protects a self-hosted runner that reuses a
  workspace).
- **Auth can never be interactive** — provide the agent credential as a
  repository secret (`ANTHROPIC_API_KEY` or an OAuth token) and `GH_TOKEN` for
  `gh`. There is no browser to log in with; a missing secret surfaces as
  ralphy's clear auth-failure error.
- **No TTY** — use `--headless-exec`: the default execution path drives an
  interactive PTY session, which CI runners don't provide.
- **Best-effort timing** — `schedule` firings can be delayed or dropped under
  load (GitHub documents this); don't build assumptions on exact ticks. Keep
  `workflow_dispatch` for manual testing.
- **Log capture** — upload `.ralphy/runs/` as an artifact with `if: always()`
  so failed runs keep their logs.
- **Permissions** — the default `GITHUB_TOKEN` may be read-only depending on
  repo settings; the run pushes branches and edits issues, so grant
  `contents: write` and `issues: write` explicitly (or use a PAT).

### Verify it worked

Check the workflow run list for the scheduled firings; a deferred firing (only
possible on a reused self-hosted workspace) logs
`skipped: run in progress since <time>, pid <X>` and still shows green.

---

## Triage-first: two phases in one window (ADR-0017)

When you use the agent-triage ramp (`triage-agent` → `ralphy triage`), run triage
*before* the run so tonight's promotions join tonight's queue. Chain the two with
the **external** scheduler, deliberately with `;` (not `&&`):

```sh
ralphy triage --repo <path> --yes ; ralphy run --repo <path> --deadline-hours 8
```

`ralphy triage --yes` publishes and promotes directly — the trust act already
happened when the operator applied `triage-agent`, so unattended promotion is a
mechanical continuation of a human decision, not the agent expanding its own
authority. The bounce arm never needs confirmation in either mode.

The `;` (not `&&`) is intentional: a broken triage costs only the triage, never
the night's execution of issues already `ready-for-agent`. Ralphy stays "the run,
not the cron" — non-users pay nothing, and each phase is a plain, independently
schedulable command. Drop the `ralphy triage` line from any recipe above that you
do not use the triage ramp for.

---

## Follow-up

A `ralphy schedule install|status|remove` subcommand that registers the timer
natively is the planned next step; these recipes double as its per-platform
specification. Its command surface, correct-by-construction defaults, the
`--with-triage` chained timer, and the repo-scoped-lock safety prerequisite are
decided in [ADR-0026](./adr/0026-native-scheduling-command.md); GitHub Actions
stays a recipe here, not a native `install` target.
