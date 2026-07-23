# Ralphy 🌙

[![Built with Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform: Windows | Linux | macOS](https://img.shields.io/badge/platform-Windows_%7C_Linux_%7C_macOS-0078D6)](https://github.com/paulocorcino/ralphy/releases)
[![License: GPL v3](https://img.shields.io/badge/license-GPLv3-blue)](LICENSE)
[![Powered by Claude Code](https://img.shields.io/badge/powered_by-Claude_Code-d97757)](https://claude.com/claude-code)

**Ralphy works through your GitHub issues while you sleep — and hands you a branch to review in the morning. ☕**

You tag the issues you trust a coding agent to handle. Overnight, Ralphy takes them one by
one: it **plans** the work, lets a coding agent **write the code**, **commits** it, and
**closes** the issue once the tests pass. In the morning you skim the branch and merge
what you like.

Three things worth knowing up front:

- 🔒 **It never pushes and never opens a PR.** Everything stays on one local branch. *You*
  review and *you* merge — Ralphy never touches your remote.
- 💳 **No API key, no per-token bill.** It runs on the **subscription** you already pay for
  (Claude, ChatGPT/Codex, and more).
- 💻 **Windows, Linux, and macOS.**

```text
  🌆 You, before bed              🌙 Ralphy, overnight            🌅 You, in the morning
┌────────────────────────┐     ┌────────────────────────┐     ┌────────────────────────┐
│  tag the issues you    │ ──▶ │  plan → code → commit  │ ──▶ │  review the branch,    │
│  trust an agent to do  │     │  → close, one by one   │     │  merge what you like   │
└────────────────────────┘     └────────────────────────┘     └────────────────────────┘
```

---

## 🤔 What is Ralphy?

Think of Ralphy as a **tireless junior teammate** who picks up small, well-described tasks
from your issue tracker and works them while you're away — carefully, one at a time, and
always leaving the final say to you.

It doesn't replace you. It does the *legwork*: reading the codebase, planning a change,
writing it, running the tests, and closing the ticket when everything's green. What it
delivers is a branch full of finished work for you to review — never a surprise on your
main branch.

## 🔁 What is the "Ralph loop"?

The idea behind Ralphy is a simple, repeating loop:

> **plan → execute → commit → verify → repeat**

Point an AI coding agent at a task, let it plan and do the work, commit the result, check
that it actually passes — then move to the next task and do it all again. Run that loop
unattended over a whole backlog and you wake up to a pile of done work.

That pattern is [Geoffrey Huntley](https://ghuntley.com/ralphy/)'s "Ralph" technique.
Ralphy is a careful, batteries-included implementation of it: a single binary that runs the
loop over your **real GitHub issues**, with guardrails so it's safe to leave running while
you sleep.

---

## 🛠️ Set up Ralphy

Three steps: get the binary, make sure you've got the basics, and initialize your project.

### 📦 Step 1 — Get the `ralphy` binary

Grab the archive for your platform from the
[**Releases page**](https://github.com/paulocorcino/ralphy/releases) — Windows, Linux, or
macOS (Intel & Apple Silicon) — and unzip it anywhere.

Then let Ralphy put itself on your `PATH` so you can type `ralphy` from any folder:

```bash
./ralphy install
```

*(Prefer to build from source? See [docs/BUILDING.md](docs/BUILDING.md).)*

### ✅ Step 2 — The basics you'll need

- 🐙 **A GitHub account and the `gh` CLI, logged in.** Ralphy works your GitHub issues, so
  it talks to GitHub through `gh`. Check with `gh auth status`.
- 🤖 **A coding-agent CLI, signed in to its subscription.** This is the "brain" that writes
  the code. [Claude Code](https://claude.com/claude-code) is the default; Codex, OpenCode,
  and others work too. → [Which agents, and how to pick one](docs/agents.md)

No API keys anywhere — Ralphy rides on the subscription you already log into.

### 🚦 Step 3 — Initialize your project

From inside your project folder, run the guided setup:

```bash
ralphy init
```

It checks your environment, creates the issue labels Ralphy uses, and gets the repo ready
to be worked. Follow the prompts — it explains each step as it goes.
[Full walkthrough →](docs/getting-started.md)

---

## 💡 Turn an idea into a backlog

Ralphy works *issues* — so first you need some. The easiest way is to let your coding agent
turn a rough idea into a clean, labeled backlog for you.

Inside your agent (Claude, Codex, …), go from fuzzy to ready in three moves:

1. 📝 **Describe your idea.** Co-author a short doc with the agent so it really understands
   what you want to build. → use the **`grill-with-docs`** skill
2. 📋 **Turn it into a spec.** Shape that doc into a proper PRD (a product requirements
   document). → use the **`to-prd`** skill
3. 🧩 **Break it into work.** Split the PRD into small, independent GitHub issues, each
   tagged so Ralphy knows it's fair game. → use the **`to-issues`** skill

`ralphy init` can set these engineering skills up for you. The result: a tidy backlog of
bite-sized issues, ready for the overnight run.

---

## 🏷️ The labels (meet AFK & HITL)

Ralphy decides what to touch purely from **issue labels**. Two matter most:

- 🟢 **`AFK`** (or `ready-for-agent`) — *"away from keyboard, agent go."* This issue is
  yours to work, Ralphy. Plan it, code it, close it when green.
- 🔴 **`HITL`** (or `ready-for-human`) — *"human in the loop."* Hands off. This one needs a
  person; Ralphy never touches it.

That's the whole mental model: tag an issue **AFK** and it joins the overnight queue; leave
it **HITL** (or unlabeled) and it's ignored. A couple more labels fine-tune things (triage,
staged plans, "stop before this one") — but AFK and HITL are the two you'll use every day.
[The full label rules →](docs/adr/0016-queue-label-precedence.md)

---

## 🌙 Run it

Here's the golden rule: **build up trust one step at a time.** Try one issue as a dry run,
then one for real, and only then let it loose on the whole queue overnight.

```bash
# 1️⃣  Plan one issue — no code changes, no commits. Then read .ralphy/plan.md.
ralphy run --only-issue 13 --dry-run

# 2️⃣  Now actually do that one issue. Commits land on a fresh afk/run-<stamp> branch.
ralphy run --only-issue 13

# 3️⃣  The real deal: work the whole queue overnight, with an 8-hour budget.
ralphy run --deadline-hours 8
```

💡 Run these from inside your repo. Pointing at a repo elsewhere? Add
`--repo /path/to/repo`.

Under the hood, for each issue Ralphy: 📝 **plans** → ⌨️ **executes and commits** → ✅
**re-runs the tests itself** → 🎉 **closes the issue** if they pass. If an issue gets stuck
or a test fails, it **stops the whole run** and hands you the branch as-is — one bad issue
can never burn the rest of the night.

### ⭐ The command you'll type most

Once you trust it, this is the everyday shape of a run:

```bash
ralphy run --agent <agent> --branch-mode <current|new>
```

Two knobs do the heavy lifting:

- 🤖 **`--agent <agent>`** — *who writes the code.* Pick the coding agent for this run:
  `claude` (the default), `codex`, `opencode`, and more. Same issues, different brain.
  → [see all agents](docs/agents.md)
- 🌿 **`--branch-mode <current|new>`** — *where the commits land.*
  - **`new`** (default) — cut a fresh `afk/run-<stamp>` branch and commit there, leaving the
    branch you're on untouched. Safest: your work is quarantined until you review it.
  - **`current`** — commit straight onto the branch you're already on. Handy when you've
    made a branch yourself and want Ralphy's work to continue right on it.

  Either way, Ralphy refuses to start on a dirty repo — so nothing uncommitted is ever at
  risk.

📖 Every other flag — deadlines, planning models, running a specific set of issues, stopping
before one, and more — lives in the [**run options reference**](docs/run-options.md).
`ralphy run --help` prints the same list in your terminal.
⏰ Want it on a timer (nightly, hourly)? → [docs/scheduling.md](docs/scheduling.md)

### 🌅 The morning after

```bash
# See what landed overnight
git log --oneline origin/main..afk/run-<stamp>
git diff origin/main..afk/run-<stamp>

# 👍 Happy? Merge it.        # 👎 Not happy? Just delete the branch —
git checkout main            #     your main was never touched.
git merge afk/run-<stamp>    git branch -D afk/run-<stamp>
```

If the run stopped early, your repo is left on the run branch so you can fix the stuck
issue in place and pick up where it left off.

---

## 📱 Keep an eye on it from your phone (optional)

Since a run is unattended, Ralphy can post a live **status card** to a Telegram chat and
keep it updated the whole way through — planning, coding, and the final summary. It's
read-only; the bot just tells you how things are going.

```bash
ralphy telegram setup    # store your bot token, then send /start to link your chat
ralphy telegram test     # send a ping to confirm it works
```

[More on the Telegram monitor →](docs/telegram.md)

---

## 💡 More you can do

Everything below is optional — reach for it when you need it.

| Feature | What it's for | Start here |
|---|---|---|
| 🤖 **Choose your agent** | Claude, Codex, OpenCode, and more — even plan with one, code with another | [docs/agents.md](docs/agents.md) |
| 🔍 **The verify gate** | why "green" means *the tests actually passed*, not *the agent said so* | [docs/verify-gate.md](docs/verify-gate.md) |
| 📊 **Cost reporting** | see how many tokens each run used, with a $ estimate | [docs/usage-and-cost.md](docs/usage-and-cost.md) |
| ⚙️ **Persistent settings** | stop retyping the same flags every run | [docs/configuration.md](docs/configuration.md) |
| ⏰ **Scheduled runs** | drain the queue nightly on a timer | [docs/scheduling.md](docs/scheduling.md) |
| 📡 **Event streaming** | POST every run event to a dashboard or webhook | [docs/events.md](docs/events.md) |
| 🧠 **Knowledge cache** | Ralphy remembers hard-won setup facts across runs | `ralphy consolidate --help` |

---

## 🛡️ Why it's safe to leave running

Ralphy is built to run while you're asleep, so it ships its own guardrails:

- 🧹 **Won't start on a dirty repo** — your uncommitted work is never at risk.
- 🚫 **Never pushes, never opens a PR** — it only commits locally. You deliver.
- ⏱️ **Time budgets** — a hung issue can't run forever.
- 🛑 **Stops at the first failure** — one stuck issue ends the run instead of burning the
  whole night.
- ✅ **Runner-enforced tests** — an issue closes only when Ralphy *itself* watched the tests
  pass. → [docs/verify-gate.md](docs/verify-gate.md)
- 🧯 **Command guardrails** — destructive commands like `git push` and `reset --hard` are
  blocked mid-run.

---

## 🙏 Credits

- **The Ralph loop** — the unattended plan-execute-commit pattern is
  [Geoffrey Huntley](https://ghuntley.com/ralphy/)'s.
- **Triage vocabulary** — the labels (`ready-for-agent`, `ready-for-human`, …) are
  **[Matt Pocock](https://github.com/mattpocock)'s**, from his
  [engineering skills](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills).

## 📄 License

GPLv3 — see [LICENSE](LICENSE). Copyright (C) 2026 Paulo Corcino.
