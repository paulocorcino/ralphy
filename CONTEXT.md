# Ralphy

A global, in-place runner that works a repo's GitHub issue queue unattended on a
Claude subscription, committing each issue and handing back a branch to merge by
hand. Its triage vocabulary follows Matt Pocock's canonical roles — the full set
of five and Ralphy's stance on each is in [docs/triage-roles.md](./docs/triage-roles.md).

## Language

**Run**:
One invocation of the runner over a repo's queue.
_Avoid_: session (one issue's agent execution inside a run).

**Queue label**:
The label that puts an open issue into the run: `ready-for-agent`, or its synonym `AFK`.
_Avoid_: todo, backlog.

**Human label**:
The label that marks an issue as work for a human only: `ready-for-human`, or its synonym `HITL`.
_Avoid_: blocked, manual.

**Green**:
An issue whose execution finished cleanly, as opposed to one that stopped as blocked, timed out, stuck or at a usage limit.

**The cycle / close-on-green**:
The runner closing a green queue issue so that it leaves the queue, while the human still merges the branch.

**Acceptance ledger**:
The per-issue list that pairs each acceptance criterion of an issue with a verdict (*verified* or *review-only*) and the evidence behind it. It is the record of how the delivered work maps back to what the issue asked (ADR-0011).
_Avoid_: acceptance check (sounds like a gate), checklist.

**Evidence (close handoff)**:
What the runner writes onto a closed green issue beyond the close itself: the ticked criteria and a comment that gives each criterion its verdict and proof from the **acceptance ledger**.

**Run branch**:
The branch where a run's commits land: a new branch for the run, or the branch the repo is already on.

**Change set**:
A repo's working-tree changes as one ordered list of paths, each with its kind of change (modified, added, deleted, renamed, untracked or conflicted). It is Ralphy's single definition of a dirty working tree.
_Avoid_: diff, status, dirty list.

**Run artifact**:
A path under the repo-root `.ralphy/` folder, which Ralphy keeps out of the **Change set** by its own rule.
_Avoid_: ignored file.

**Ignored path**:
A path that the repo's own git ignore rules keep out of tracking. It is a fact of the local working tree, not of the **Forge**.
_Avoid_: hidden, excluded.

**Ignored mark**:
The sign the workbench file tree draws on an **Ignored path**.
_Avoid_: gitignore status.

**Sync status**:
Where a repo's branch stands against its upstream: the current branch (or commit, when detached), the upstream it tracks, and how far ahead or behind it is, as of the last fetch.
_Avoid_: sync, remote state, tracking info.

**Working-tree operations**:
The four acts on the **Change set** of one working tree: **stage**, **unstage**, **commit** and **discard**.
_Avoid_: add, index write, save, checkpoint, revert.

**Checkout**:
A second working tree of a repo that Ralphy created under `.ralphy/worktrees/`, on a branch of the same name. The operator sees it as a "worktree"; the registered tree is the **primary** (ADR-0063).
_Avoid_: "checkout" for switching a branch (that is a **branch switch**).

**Adapter**:
The unit that holds everything specific to one agent CLI vendor, behind the core's agent contract (ADR-0002).
_Avoid_: driver, plugin, backend.

**Planner / Executor (phase roles)**:
The two roles an **adapter** fills in a run: the *planner* writes the plan from the issue, and the *executor* carries out the plan and commits.
_Avoid_: stage, role (overloaded with triage roles).

**Split run**:
A run whose **planner** and **executor** are different **adapters** (ADR-0009).
_Avoid_: mixed-vendor, planner override.

**Settings**:
The per-repo operator configuration that `ralphy config` reads and writes (ADR-0010).
_Avoid_: config (the subcommand), preferences, dotfile.

**Event sink**:
A consumer of the run's structured events, such as the console, the run log, the Telegram notifier or the CloudEvents sink (ADR-0019).
_Avoid_: exporter, webhook (the sink pushes; it exposes nothing), logger.

**Emitter identity**:
The attributes on every run event that tell one running Ralphy apart from another: the run id as the key, plus who, where and which version.
_Avoid_: instance id (the daemon's identity is a different thing), session id.

**Queue snapshot**:
The backlog of a repo as the runner judges it: each issue with its queue status, skip reason, blockers and position (ADR-0020).
_Avoid_: backlog dump, issue list (the forge's raw list, without judgment).

**Run snapshot**:
The live state of one **run**, which the run itself publishes and replaces as it changes. It is state, not a log (ADR-0047).
_Avoid_: run event, run log (the event sink's history), **queue snapshot** (the backlog, with no run), heartbeat, run history.

**Adapter support**:
The code that every **adapter** shares and that belongs to no single vendor: driving a headless child, reading the done and blocked sentinels, and placing skills where a vendor CLI finds them. It is the counterpart of an **adapter**, which holds what is vendor-specific (ADR-0002).
_Avoid_: shared runner, headless runner (no shared outcome runner exists), utils, helpers.

**Run deadline / per-issue budget / idle watchdog**:
The three separate clocks that bound a **run**. The **run deadline** is the wall-clock budget of the whole run, the **per-issue budget** is an optional cap on one issue, and the **idle watchdog** is the limit on how long an agent may make no progress (ADR-0038).
_Avoid_: "the timeout" (it could mean any of the three).

**Completion signals / Outcome classifier**:
The **completion signals** are the facts an **adapter** reads from the end of an agent session: done, blocked, limit, committed, timed out, exited cleanly, errored. The **outcome classifier** is the one vendor-neutral rule that turns those signals into the outcome of an issue (ADR-0023).
_Avoid_: completion protocol (that is the sentinel reading in **adapter support**), outcome mapping.

**Execution mode** (interactive vs headless):
How an **adapter** drives its vendor CLI: interactive, inside a terminal (PTY), or headless, as a program with no terminal. It is a concern of the adapter and of billing, never of the core (ADR-0002).
_Avoid_: -p mode, batch.

**Complexity routing**:
An optional **adapter** capability in which the planner judges how complex an issue is and picks the execution model from that judgment. It is a choice of model, unlike **effort**, which the operator sets.
_Avoid_: model selection (too broad), auto-model.

**Effort**:
How hard the agent reasons in one phase, set by the operator on one Ralphy ladder: `low`, `medium`, `high`, `xhigh`, `max`. Each **adapter** translates it into its vendor's own setting (ADR-0044).
_Avoid_: reasoning level (a vendor word), variant (OpenCode's word).

**Supervised session**:
A person following a running agent session live and able to step in, from a phone or an on-screen terminal. It is different from the **Human label**, where the agent never works the issue at all.
_Avoid_: HITL (reserved for the triage role), human-in-the-loop.

**Daemon**:
The resident mode of the `ralphy` binary (`ralphy daemon`): it starts runs and hosts **workbench sessions**, but a run's own loop never runs inside it. There is one daemon per environment; a WSL distro is its own environment (ADR-0032).
_Avoid_: service, server (it dials out), agent (reserved for vendor CLIs), instance id.

**Daemon identity**:
The three names of one **daemon**: a stable id that machines use, a unique name that people and models use, and an emoji avatar that is only for display (ADR-0032).
_Avoid_: hostname (a suggestion for the name, not the name), token (the credential is per daemon).

**Device**:
One browser profile on one machine that talks to a **daemon**, known by an ID the daemon issues in a signed cookie. It is a record for the **audit log**, never a credential (ADR-0074).
_Avoid_: client (also the HTTP client), fingerprint (computed in the browser), user (there is one operator), session (a login of a device).

**Device facts**:
What a page reports about its own browser, screen and machine, with what the daemon read from the request headers. Each fact keeps its source, because a page can report false facts (ADR-0074).
_Avoid_: fingerprint, telemetry (Ralphy sends nothing out), user agent (one of the facts).

**Audit log**:
The daemon's record, one line per event, of each login, logout, **device facts** report and request that changed state, with the **device** that sent it (ADR-0074).
_Avoid_: history (the **desk history** is a different thing), access log, event log.

**Fleet**:
The set of **daemons** one operator commands through the **control plane**, across machines and environments. A Windows host and its WSL distro are two members of the fleet.
_Avoid_: cluster (no shared workload), farm.

**Local fleet**:
The **daemons** that the **local daemon** (the one serving the browser) reaches over its own loopback, with no **control plane** in the path. Each of the others is a **peer**, on the same machine through WSL or on another machine through a **peer tunnel** (ADR-0052).
_Avoid_: remote daemon (a peer on another machine is still a peer), master/slave or primary, mount, share, remote peer.

**Peer descriptor**:
The file a **daemon** writes into a **peer**'s store to announce itself: who it is, where to reach it, how to authenticate, and which protocol version it speaks. It is a claim, not a proof that the daemon is alive (ADR-0052).
_Avoid_: enrollment (the control plane's code exchange), service discovery, pairing.

**Nudge**:
A request, sent and not followed up, that an environment start its own **daemon**, for example asking a WSL distro to start the daemon's service. The daemon that nudges does not own the daemon it wakes (ADR-0052).
_Avoid_: spawn, launch (both mean a parent that owns the child), remote exec.

**Keepalive**:
An idle process that a Windows **daemon** holds in each WSL distro, so that WSL does not stop the distro where a **peer** runs. It keeps the distro open; it does not supervise anything (ADR-0052).
_Avoid_: watchdog, supervisor, heartbeat (nothing is checked or restarted).

**Peer tunnel**:
The `ssh` port forward a **daemon** holds for each **peer** on another machine, so that the peer answers on this machine's loopback (ADR-0067).
_Avoid_: control-plane tunnel (the connection to the hosted control plane), connection, link, VPN.

**Peer key**:
The SSH key a **daemon** creates for its machine and uses for every **peer tunnel** it opens (ADR-0067).
_Avoid_: credential (the daemon's access token), password vault, keychain.

**Forge**:
The service that hosts a repo's remotes, issues and labels. Today that is GitHub only; the neutral word names contracts that could face another forge later.
_Avoid_: provider (vague), host (used by **Emitter identity**), platform (the **control plane**'s word).

**Forge query**:
A read-only question the **control plane** asks a **daemon** about a repo's forge data (issues, an issue's thread, labels, branches), answered with the operator's own forge login. Its verbs use Ralphy's words, not the forge's (ADR-0032 §6).
_Avoid_: gateway, proxy, GitHub query, graph.

**Session store**:
A vendor CLI's own record on disk of every session it ran, whether a run or a person started it. It belongs to the vendor, which deletes old sessions on its own schedule.
_Avoid_: transcript (Claude's store only), logs, history.

**Interactive usage**:
The tokens spent in agent sessions that a person drives directly, outside any **run**, including **workbench sessions** and **supervised sessions**. It is found only in the **session stores**, never in the ledger (ADR-0033).
_Avoid_: invisible tokens, proxy capture, manual usage.

**Usage scan**:
A read of the **session stores**, from nothing each time and keeping no state, that answers how many tokens runs and **interactive usage** spent in one environment (ADR-0033).
_Avoid_: harvester (nothing runs in the background), proxy, telemetry, collector.

**Tokens**:
The one vendor-neutral token count every collection path produces: input, output, cache read and cache creation. The model is not part of it; it is recorded next to it.
_Avoid_: Usage (the old name), TokenBreakdown, reasoning as a fifth count, counts.

**Priced usage**:
An estimate in USD of **Tokens**, made each time a report is read, from the provider and model of the record and the price table. It is never stored (ADR-0034).
_Avoid_: cost (too generic), ghost cost (the vendor's own figure), stored cost.

**Delivery (entrega)**:
One issue, seen as the unit that project spend is divided by. Its cost is the run spend on that issue across every attempt and every run.
_Avoid_: run (a run works many deliveries), PR or branch (the hand-off, not the unit), task, ticket.

**Retry burn**:
The part of a project's spend that bought no **delivery**: the spend of the attempts that did not succeed.
_Avoid_: waste, overhead (that is **interactive usage**), failure rate (counts attempts, not money).

**Unpriced volume**:
The tokens a spend total could not price, shown beside the total so that the total reads as a lower bound. It is either tokens of a model the price table does not know, or tokens of a ledger line that never recorded its model.
_Avoid_: `$0`, missing cost, untracked (the tokens are tracked; only the price is missing).

**Model recovery**:
Finding the model of a ledger line that never recorded one, from the vendor **session store**, without changing the ledger (ADR-0053).
_Avoid_: backfill, migration (both mean writing to the ledger), correction.

**Repo registry**:
The list of repos one **daemon** can act on, each keyed by the repo's project
identity (`owner/repo`), with its path on disk as a detail that can change.
(ADR-0036)
_Avoid_: workspace list, auto-discovery (nothing scans the disk).

**Workbench session**:
An interactive agent CLI session that a **daemon** hosts and a person drives
from a browser terminal, on the daemon's own machine. No run is involved; it
belongs to the daemon, not to the browser connection. (ADR-0051)
_Avoid_: remote shell (only the free-console kind), terminal (the widget, not
the session), remote session, spectator mode.

**Free console**:
A **workbench session** that runs a shell, or one command line the operator
typed, instead of an agent CLI.
_Avoid_: remote shell.

**Writer slot**:
The right to type into a **workbench session**, held by one browser client at
a time. The other attached clients are **watchers**: they see the same output
and cannot type. (ADR-0051)
_Avoid_: slot without a qualifier (the **Slot (secondary pane)** is a
different thing), spectator mode.

**Clipboard drop**:
The file that an image pasted into a **workbench session**'s terminal becomes:
a run artifact under the repo's `.ralphy/`, whose path is pasted into the
prompt. (ADR-0055)
_Avoid_: upload (a different feature), attachment (the CLIs' own word),
clipboard bridge (rejected).

**File encoding (of a workbench read)**:
The text encoding the daemon used to decode a file the workbench opened, and
that a save of the same file uses again. UTF-8 is Ralphy's own encoding; any
other encoding is one a file already had. (ADR-0036)
_Avoid_: charset (the HTTP word), code page (one kind of encoding),
auto-detect (rejected).

**Unencodable**:
A save the daemon refuses because a character in the text cannot be written in
the encoding the save named. (ADR-0036)
_Avoid_: lossy save, transcoding error.

**Canvas / Consoles tab**:
The **canvas** is the central, tabbed region of the workbench. Its first tab,
the **Consoles tab**, is fixed and holds the console windows; every other tab
is an open file or a **daemon view**. (ADR-0037)
_Avoid_: view, page, screen; "main tab" for the Consoles tab; Agents tab;
panel or accordion for a sidebar view; dashboard, modal or overlay for a
daemon view.

**Daemon view**:
A canvas tab that shows a document the daemon serves, with no file behind it
and nothing to save, such as the Spend tab. (ADR-0037)
_Avoid_: dashboard, modal, overlay.

**Stage / viewport**:
The **stage** is the plane on the **Consoles tab** that the console windows,
**fences** and **cards** live on. The **viewport** is the fixed box the
operator looks at the stage through, and moving it is a **pan**. (ADR-0051)
_Avoid_: canvas (the tabbed region one level up), zoom, clamping or refitting,
infinite canvas (the stage is finite).

**Fence**:
A named rectangle on the **stage** that gives a region of the plane a meaning,
such as "backend" or "planning". It is part of the **desk layout** and is
never bound to a project. (ADR-0051)
_Avoid_: group, zone, region, container, swimlane, project fence.

**Locked**:
The state of a console window or a **fence** that the operator pinned in
place, so it cannot be moved or resized. It is part of the **desk layout**.
(ADR-0050, ADR-0051)
_Avoid_: pinned (the stage's origin is what is pinned), frozen, read-only (the
console stays live), maxlock (unrelated).

**Focused fence**:
The one **fence** a browser client is working in: the fence that a new
console opens inside. It is per client and is not part of the **desk
layout**. (ADR-0051)
_Avoid_: selected fence, active fence, current fence.

**Detached fence**:
A **fence** whose consoles one browser tab shows in a separate popup window,
for example on a second monitor. The fence stays on the stage, and the daemon
and other browsers do not see the detach. (ADR-0051)
_Avoid_: popped-out or floating fence, undocked, mirrored fence.

**Note**:
A markdown document the operator writes for themselves, stored as one `.note`
file in a **checkout**. The file is opaque to an agent that reads by accident,
but it is not secret. (ADR-0064)
_Avoid_: sticky or post-it (that is the **card**'s look), memo, review note
(an annotation on a diff, ADR-0061), secret or encrypted.

**Card**:
A **note** shown on the **stage**. The card is its place in the **desk
layout**; the content stays in the note's file. (ADR-0064)
_Avoid_: note window (a window belongs to a session), widget, tile.

**Card on top**:
A **card** the operator took off the **stage** for a while, so that it floats
in front of the console windows. Its place on the stage stays as a **shadow**.
(ADR-0064)
_Avoid_: note on top (the file does not move), floating card, pinned note,
detached note (detach is the fence's popup).

**Slot (secondary pane)**:
The canvas's one optional second pane, beside the active tab. It holds a
**pin** (another open tab) or a **mirror** (the same code tab in a second
editor), and it is part of the **per-client view**. (ADR-0037)
_Avoid_: split editor, editor group, second tab, writer slot (a different
thing).

**Columns**:
A maximized console and the consoles the operator opened beside or below it,
shown as columns of equal width that fill the **viewport**. Each console in a
column is a **row**. It is part of the **per-client view**. (ADR-0051)
_Avoid_: split view, split (a **split run** is a different thing), focus mode
(a **focused fence** is a different thing), tile (the fence's arrange verb),
group, editor group, pane without a qualifier; a terminal's cols or rows.

**Console name**:
The name a person reads for one console window, such as `fincal #1` or
`backend`. It belongs to the console, not to the session, and it is not an
identity. (ADR-0066)
_Avoid_: title, session name (that dies with the session), alias, tag, label
(the agent or command shown beside the name).

**Shown fact**:
A fact the workbench shows whose owner is outside the browser, such as the
session list, the **desk layout**, the branch or the board. The browser holds
only the last value it read; a new tab on another device shows the same shown
facts, but may show a different **per-client view**.
_Avoid_: workbench copy (copy means UI text), server state (the daemon is not a
server), cache, read model, state without a qualifier.

**Per-client view**:
What one browser profile was looking at in the workbench: where the view sits
on the **stage**, the open file tabs, the **slot**, and the consoles open as
**columns**. Its owner is the browser, not the daemon, so two devices can show
different per-client views of the same **desk layout** (ADR-0051).
_Avoid_: browser desk, geometry store, session state.

**Desk layout**:
The daemon's record of which console windows were open on the **Consoles tab**,
with each window's repo, agent, kind, place on the **stage** and state. It is
what the workbench restores when a page opens (ADR-0050).
_Avoid_: workspace (that is the **viewport**), geometry store, browser state.

**Desk change**:
One change a page sends to the **desk layout**: create, set some fields of,
or remove one window, fence or note card, or select or clear a project's
checkout. The daemon applies them in the order they arrive (ADR-0050).
_Avoid_: patch, delta, diff, desk upload (that is the whole request).

**Desk history**:
The last 50 versions of the **desk layout** that the daemon keeps, so the
operator can restore one, or save it as a file (ADR-0050).
_Avoid_: backup, snapshot (that is a **Run snapshot** or a **Queue snapshot**), undo.

**Key bar**:
The row of keys under a console window that supplies the keys a tablet's
on-screen keyboard does not have, such as `esc`, `tab`, `ctrl` and the arrows.
It is a way to type into the console, not a menu.
_Avoid_: toolbar (it types, it does not command), soft keyboard (that is the
operating system's).

**Resume**:
Reconnecting a page's connections to the daemon after the browser suspended the
page, for example on a tablet. It is about this page, not about another machine.
_Avoid_: wake (reserved for a sleeping **peer**), reconnect (the mechanism, not
the trigger).

**Adapter roster**:
The daemon's list of the **adapters** it can launch, with whether each vendor
CLI is present in that daemon's environment. The workbench's console menu is
built from it (ADR-0052).
_Avoid_: agent list, capabilities (availability is presence in one
environment, not a capability model).

**Seed**:
Made-up data that exists only so the static demo of the workbench, opened as a
file with no daemon, has something to show. It is never real data and never a
fallback for a daemon that did not answer.
_Avoid_: mock, fixture (a fixture is a test input), fallback (it is never one).

**Control plane**:
The planned single web application where the **fleet** meets: it receives run
events, relays **control-plane tunnels**, answers the Telegram commands, and
shows the fleet with browser terminals. It is not built yet (ADR-0032).
_Avoid_: dashboard (it commands, not just displays), relay (one of its roles),
events platform (subsumed).

**Control-plane tunnel**:
The one lasting outbound connection each **daemon** holds to the **control
plane**, carrying console streams, commands and a presence signal. It carries
only interactive traffic, not run events (ADR-0032).
_Avoid_: webhook, event channel (that is the sink), gateway (the daemon is not
a server).

**Blocked by / dependency gating**:
The other issues an issue names in its `## Blocked by` section as work that must
be done first. An issue with an open blocker waits for a later run (ADR-0045).
_Avoid_: depends-on, prerequisite, stop-before (that is flow control, not a
dependency).

**stop-before**:
A fixed control label on one queued issue that ends the run just before that
issue. It controls the flow of the queue; it is not a triage role (ADR-0001).
_Avoid_: pause, hold, breakpoint.

**Cooperative stop**:
An operator's request that a live **run** end where it is, which the run itself
carries out. The issue in flight stays open (ADR-0054).
_Avoid_: kill (nothing is signalled from outside), abort, cancel, interrupt,
pause (that is **stop-before**).

**Init / onboarding**:
The interactive command, `ralphy init`, that prepares a repo so that
`ralphy run` can work it: environment checks, files, **labels**, skills, and
issues made from an existing backlog (ADR-0012).
_Avoid_: setup (taken by the setup-pocock skill), bootstrap, scaffold (only one
stage of init).

**Repo diagnosis**:
The read-only first agent pass of **init**: a report of what the target repo
already has, such as a backlog, agent skills, domain docs and a remote. It
fills in the defaults of the init questions (ADR-0012).
_Avoid_: scan, audit (reserved for security and review), analysis.

**Changelog fragment**:
The one file a pull request adds to say what its change means to a user, with a
`kind` from a closed set and a short sentence that names the capability. It is
the only release text a person writes (ADR-0056).
_Avoid_: changelog entry (that is the folded output), release note (the whole
body), news file, towncrier fragment.

**Release watch**:
The daemon's read of Ralphy's published releases, which tells the workbench
whether the running build is behind, level with, or ahead of the newest
release (ADR-0056).
_Avoid_: update check, auto-update (nothing is ever unattended), telemetry
(nothing is sent), version ping.

**Channel**:
The stream of releases a Ralphy follows when it looks for a newer one: `rc`,
which includes release candidates, or `stable` (ADR-0056).
_Avoid_: track, branch (taken by git and by the **run branch**), ring, stream.

## Relationships

- The **queue** is the open issues that carry a **queue label**, in ascending
  issue number. A **human label** issue is never in it.
- A **green** queue issue is closed by the runner (**the cycle**); a non-green
  one stops the run and hands back the **run branch**.
- A **stop-before** issue stops the run before itself; a **cooperative stop**
  stops it wherever it is. Same word, different things.
- A **blocked-by** issue with an open blocker is skipped, not stopped; later
  issues still run.
- Closing a green issue writes its **acceptance ledger** back as **evidence**,
  without changing what makes it **green**.
- The core asks an **adapter** to work an issue and receives an outcome. The
  **execution mode** lives inside the adapter, never in the core.
- A **daemon** launches runs but never contains them. A run started by hand is
  the same run as one the daemon started.
- A **run snapshot** says what is happening now; the **event sink** says what
  happened. Neither is derived from the other.
- A **workbench session** involves no run; a **supervised session** watches
  one. The tokens either one spends are **interactive usage**.
- A **delivery** is one issue; **retry burn** and **unpriced volume** are read
  beside the spend of deliveries, never folded into it.
- A **shown fact** has its owner outside the browser; the **per-client view**
  is owned by the browser.
- A **changelog fragment**'s kind, not the version number, decides how the
  **release watch** presents a release.

## Flagged ambiguities

- "AFK" and "ready-for-agent" are synonyms, as are "HITL" and
  "ready-for-human". The canonical word is the Matt Pocock role; the short
  form is a transitional alias.
- "HITL" was used for both the **human label** and live human oversight of a
  running session. Resolved: HITL is only the label; oversight is a
  **supervised session**.
- "Fleet" was used for both concurrent runs and the set of enrolled daemons.
  Resolved: **fleet** means the enrolled daemons; runs are told apart by their
  run id, never by fleet membership.
- "Session" names three things: the vendor's session (one CLI conversation in
  its **session store**), the daemon-hosted **workbench session**, and the
  **supervised session**. It never means one issue's execution inside a
  **run**.
- "GitHub" and "forge": **forge** is the neutral word, for contracts that must
  not depend on one vendor; GitHub is today's only forge, and prose that names
  it where it means that forge is correct. A repo with no remote has no forge,
  so the queue, labels and forge queries do not apply to it.
- "Worktree" and "checkout": "worktree" is the operator's word; a
  **checkout** is a worktree that Ralphy created. "Checkout" never means
  switching a branch; that is a **branch switch**.
- "Ignored" was used for four things. Resolved: an **ignored path** is git's
  answer from the repo's ignore rules; a **run artifact** is kept out of the
  change set by Ralphy's own rule; the folders the file tree never lists are
  neither; and the workbench search always includes `.ralphy/`. Which files
  are ignored is a fact of the local working tree, never of the forge.
