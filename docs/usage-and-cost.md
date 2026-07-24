# Usage limits and cost reporting

Ralphy runs on a subscription, not a metered API key — so there's no dollar cap to set. But
it still **measures** what each run consumes so you can see how efficient a task was.

## Usage limits

There's no dollar cap to set — there's no API spend. On **Claude** and **Codex**, when you
hit a usage limit Ralphy **waits for the reset and resumes the same issue** automatically
(pass `--stop-on-limit` if you'd rather it stop and report). Both emit a trustworthy reset
time — Codex an absolute timestamp, Claude a relative one. **Kimi** and **OpenCode** always
stop and report — re-run once the limit clears. (Kimi keys the limit off the CLI's exit
code 75, so there's no reset timestamp to wait on; the stop is forced.)

When planner and executor are split, usage-limit handling is **per-phase**: a Claude planner
can wait out a plan-time reset while the OpenCode executor stops on an execute-time limit.
An explicit `--stop-on-limit` forces both phases to stop.

## Cost reporting

You don't pay per token, but Ralphy still **measures** what each run consumed so you can
see how efficient a task was. Every run harvests the token counts each agent CLI already
reports and accumulates them durably per project in an append-only ledger
(`.ralphy/usage.jsonl`). The end-of-run footer shows the run total and the project's
cumulative balance as a token meter (`↑` input, `⚡` cache write, `❄` cache read,
`↓` output) plus a read-time USD estimate priced per model (`~$?` when a model has no
known price). USD is only ever a read-time projection — it never enters the ledger, so
re-pricing never rewrites history.

Read the ledger after the fact with `ralphy usage`:

```powershell
ralphy usage                       # the project balance: total tokens + estimated USD
ralphy usage --by model            # group by model (also: phase, actor, version)
ralphy usage --since 2026-06-01    # only rows on/after a date
ralphy usage --format csv          # export (also: json) instead of the table
ralphy usage --project owner/repo  # read another project's ledger
```
