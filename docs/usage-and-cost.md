# Usage limits and cost reporting

Ralphy runs on a subscription, not a metered API key — so there's no dollar cap to set. But
it still **measures** what each run consumes so you can see how efficient a task was.

## Usage limits

There's no dollar cap to set — there's no API spend. When an agent hits a usage limit,
Ralphy **waits and then resumes the same issue**. This is the default for every agent.
Pass `--stop-on-limit` if you want the run to stop and report instead.

How long it waits depends on what the agent reports:

- **Claude** and **Codex** report when the limit resets. Ralphy waits until that time,
  plus five minutes.
- **Kimi**, **OpenCode**, **Copilot**, **Cursor** and **Gemini** report no reset time
  Ralphy can use. Ralphy waits about 30 minutes, tries again, and repeats until the limit
  clears.

The run deadline (`--deadline-hours`) and `ralphy stop` end a wait early. If an issue hits
a limit with a reset time twice in a row without a new commit, Ralphy stops retrying it
and reports the limit.

When planner and executor are split, each phase handles its own limit: a Claude planner
can wait for a plan-time reset, and an OpenCode executor can wait for an execute-time
limit. `--stop-on-limit` makes both phases stop.

## Cost reporting

You don't pay per token, but Ralphy still **measures** what each run consumed so you can
see how efficient a task was. Every run harvests the token counts each agent CLI already
reports and accumulates them durably per project in an append-only ledger, one file per
project under your home folder (`~/.ralphy/usage/<owner-repo>.jsonl`). The end-of-run footer shows the run total and the project's
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

Or read it in the workbench: the **Spend** button on the icon rail opens a canvas
tab scoped to the open project, showing its total and the same token meter the
terminal footer uses. The figure is priced through the same table the CLI reads,
so the two never disagree.

A total that carries volume Ralphy could not price is shown as a **floor**
(`$15.00+`), with the unpriced tokens beside it rather than folded away — `$0`
would be a lie that hides spend. That gap is split by what you can do about it:
*recoverable* (the line recorded a session id, so the engine can still be
recovered from the vendor's session store), *no price* (a real model missing from
your `pricing.toml`), and *lost* (the line recorded no session id at all — no
amount of work brings that one back).
