# Agents in a fence form a squad and talk only through letters the daemon keeps

Status: proposed
Kind: structural
Protects: integrity of change, consistency of the workbench, security

## Context

An operator already builds teams of agents by hand. On the **stage**, the
operator puts agent consoles (Claude, Codex, OpenCode…) inside a **fence**
named for a job, such as "backend" or "planning", and wants the agents in
it to work together while the operator watches. Today Ralphy has no path
for that:

- The only way into a console is the **writer slot**: a person typing in a
  browser (ADR-0051). The daemon itself never puts input into a session.
- A **run** cannot be reached at all while it runs: the daemon gives it a
  null stdin (`crates/ralphy-daemon/src/dispatch/spawn.rs`), and the only
  control is the cooperative stop (ADR-0054).
- Agent state (`working`, `waiting`, `done`) exists only for Claude, from
  its hooks (ADR-0059). The other vendors show nothing.
- The fence has a meaning, not a behaviour: which console is in which fence
  is computed only in the browser (`fenceMembership` in
  `crates/ralphy-daemon/assets/ui/wb-geometry.ts`), from the window's
  centre. The daemon stores every rect (ADR-0050) but does not compute it.
- When an agent needs the operator, each source reaches the operator a
  different way, or not at all (the dot on a console, a Telegram push for a
  run, a forge label).

Measured on 2026-10-11, on Windows, with Claude Code 2.1.296, codex-cli
0.160.0 and opencode 1.18.32 (records in
`docs/research/agent-squads-foundations-2026-10-11.md` §14):

- All three accept an MCP server over HTTP with an `Authorization: Bearer`
  header given at launch: Claude by `--mcp-config`, Codex by
  `-c mcp_servers.<n>.url` and `bearer_token_env_var`, OpenCode by
  `OPENCODE_CONFIG_CONTENT`.
- A charter can be given at launch: Claude `--append-system-prompt`, Codex
  `-c developer_instructions`, OpenCode `instructions`.
- State: Claude and Codex have hooks (Codex accepts them per launch with
  `-c` and `--dangerously-bypass-hook-trust`; its Stop hook must print
  nothing). OpenCode's TUI is a client of a local server that streams
  `session.status` (busy, idle) and `session.idle`.
- A way in without typing: Claude hooks (`additionalContext`,
  `asyncRewake`, a Stop `block`), OpenCode `POST /session/:id/prompt_async`.
  `codex queue` stores a message for a session but does not wake it unless
  the TUI runs on Codex's shared app-server daemon.
- Claude has its own messaging between sessions of the same OS user
  (`SendMessage`), with no per-sender scope. Codex and OpenCode have none.
- The OpenCode server outlived the shell that started it.

Each vendor CLI owns its agent loop. Ralphy cannot change how a loop takes
input; it can only use the doors each vendor documents.

## Decision

**D1. Two operating modes.** In *unattended* mode a **run** works the queue
and its **phase roles** talk only through **role artifacts**. In
*supervised* mode the operator runs agent consoles on the stage. A squad
exists only in supervised mode. A run never joins a squad and never gets
the squad tools.

**D2. A fence is a squad, and the daemon owns who is in it.** A console
belongs to the fence that holds the centre of its window. The test is
half-open (`left <= x < left + width`), the first fence that holds the
centre wins, and a missing rect is read at the origin; fences never overlap,
so the answer is unique. A console outside every fence is in no squad.

- **The squad fact comes only from the daemon.** The daemon computes
  membership from the stored **desk layout**. Everything that acts on a
  squad (the scope of a letter, of a console token, of `agents_list`)
  reads it there, and nowhere else.
- **The browser keeps the same rule for its gestures.** While the operator
  drags, resizes, locks, tiles or detaches, the browser applies the rule to
  the rects on its own screen, which the daemon has not stored yet. It never
  uses its own answer to show or decide squad membership; the squad shown
  on the stage is the daemon's.
- **The two copies are one rule, held by one fixture.** A table of cases
  (a centre on a shared border, abutting fences, a missing rect, a window
  in no fence) is read by the Rust test and by the browser test. A change
  to either copy that the other does not follow fails the build.

**D3. Letters are the one channel between agents in a squad, for every
vendor.** A **letter** goes from one agent console to another in the same
fence, through the daemon, which sets its sender. Inside a fence, a
vendor's own messaging between sessions is turned off where the vendor
allows it (Claude: a permission deny rule on `SendMessage` in the
per-console settings). Outside every fence, nothing about the vendor
changes.

**D4. A letter's content reaches an agent only as a tool result.** The
receiver reads its letters with an MCP tool. Each letter is wrapped with
its sender and marked as coming from another agent. Letter content never
enters a session as a user turn, a system reminder, a hook reply, or bytes
typed into a PTY.

**D5. A doorbell carries no content.** A **doorbell** is a fixed text that
Ralphy writes, saying that letters are waiting and which tool reads them.
It goes in by the vendor's documented way: a hook reply for Claude,
`prompt_async` for OpenCode, and for Codex either `codex queue` on a
Ralphy-started app-server daemon or a Stop hook `block` (the live test of
E6 chooses). For the v1 vendors the daemon never types into a PTY. A
later vendor with no documented way in may get the doorbell typed into
its PTY, only when the operator has not typed in that console for a few
seconds.

**D6. Agents reach the daemon through one MCP endpoint, with a console
token.** The daemon serves MCP over HTTP. Each agent console in a fence gets
a **console token** when its session starts. The token names the console by
its stable desk record id, not by the session id. It is written only into
that console's own vendor configuration (owner-only file or the child's
environment). It opens only the squad tools of D7, for that console's fence.
The squad routes require the token on every bind, loopback included,
because they decide who the sender is, not only who may connect. The token
dies with the session. Moving the console out of the fence takes the scope
away at once.

**D7. The squad tools are a closed set:** `agents_list` (the members of the
caller's fence, with **duty**, vendor and state), `message_send`,
`inbox_read`, `ask_human`. A new tool is a decision. The console operation
tool of D14 is separate and has its own rule.

**D8. A member is joined only when the daemon sees the wiring work.** A
member is *launching* until the daemon receives its first MCP call or its
first state event; then it is *joined*. If that does not happen, it is
*degraded* and shows why (an untrusted hook, a refused MCP server, a server
that did not start). A moved-out console is *left*; a closed one is
*ended*. At join the daemon records the vendor's session id, which is the
address for that vendor's doorbell and the key for a later resume.

**D9. Agent state has one fold and two kinds of source.** A hook file
(Claude, Codex) and an event stream (OpenCode) both become the same
vendor-neutral raw events, folded by one rule into `working`, `waiting`,
`done` or `unknown` (ADR-0059). A vendor with no source is `unknown`.

**D10. Every wait has a deadline, and a stuck squad is named.** A letter can
say it expects a reply. The daemon gives that wait a deadline (one global
setting, 10 minutes by default); a late wait is *overdue* and rings the
sender's doorbell. A fence is *active* (someone working), *needs operator*
(someone waiting for the operator), *stalled* (nobody working, nobody
waiting for the operator, and an open wait or an unread letter older than
the deadline), *idle*, or *can't tell* (a member is `unknown` or
*degraded*). *Can't tell* is never shown as *idle*.

**D11. A stalled squad goes to its lead first, then to the operator.** A
fence has at most one console with the duty *lead*; the daemon refuses a
second. A stalled squad sends a letter to the lead that names who waits for
whom. If the squad is still stalled after one more deadline, or has no
lead, the daemon opens an **attention request**.

**D12. A duty belongs to the console.** It is stored on the desk record,
like the **console name** (ADR-0066). Its charter is a file inside the repo
or under `~/.ralphy/duties/`, given to the agent at launch by the vendor's
documented way. Its description is what `agents_list` shows. A duty grants
no permission: the charter asks, the existing guard enforces.

**D13. The attention list is derived in the daemon.** An attention request
lives while its source says so: a console agent `waiting`, a run agent
`waiting`, a stalled fence, an `ask_human`. Only an `ask_human` is stored,
until it is answered or its console closes. The answer to an `ask_human`
travels as a letter (D4). A vendor's own prompt (a permission menu, a
question in its TUI) gets a "go to the console" action, not remote typing.

**D14. The console operation tool reaches only free consoles in the same
fence.** An agent may send text and named keys to a **free console** in its
fence and read its output. It may not do this to an agent console: that
would turn another agent's words into the operator's. The operator sees the
typing live and a mark on the console. The tool waits while the operator is
typing. The audit log records what the agent sent, apart from the operator's
input.

**D15. A fence has a budget.** The daemon limits letters per fence per hour
and the length of a chain of replies, and refuses `message_send` past either
limit, with the reason.

**D16. Letters and the fence timeline are append-only JSONL in the daemon
store.** One writer, the daemon. Files are rotated and every input has a cap
(ADR-0072 D11). The daemon rebuilds open waits and attention requests from
them at boot. No database in this decision.

**D17. The daemon ends the whole process tree of a vendor server it
starts**, such as an OpenCode server behind a console.

**D18. The fence scope holds for Ralphy's channel only.** Each vendor keeps
doors of its own that any process of the same OS user can use (Claude's
messaging between sessions outside a fence, `codex queue`, an OpenCode
server for whoever has its password). The workbench and the docs say so.
The squad is a way to work together, not a sandbox.

**D19. The v1 vendors are Claude, OpenCode and Codex.** A vendor joins a
squad by a checklist in the ADR-0040 inventory: S1 MCP at launch, S2
charter at launch, S3 a state source, S4 a doorbell. A vendor with S1 and S2
only may join as a *letter-only* member: no doorbell, state `can't tell`,
shown as such.

## Consequences

- **ADR-0051 is amended.** A fence gains a behaviour: its members form a
  squad, and its membership becomes a daemon fact (D2). The rule gets a
  Rust copy in the desk, and the copy in `wb-geometry.ts` stays for
  gestures, both held by one shared fixture. It is used in about twelve
  places in the browser, most of them during a gesture on rects the daemon
  has not stored yet (measured 2026-10-11: `wb-console-fences.ts`,
  `wb-console-detach.ts`, `wb-console-desk.ts`, `wb-desk-folds.ts`,
  `wb-notes.ts`, `wb-console.ts`).
- **ADR-0072 is amended.** A new trust zone: an agent in a squad. Its
  letters are untrusted data for the receiver, like an agent session's
  output. D7 of ADR-0072 gets one exception: a console token is given to the
  agent's process on purpose, scoped to D7 of this ADR. Codex hooks that
  Ralphy writes run with `--dangerously-bypass-hook-trust` for that one
  launch; no other hook gets the bypass.
- **ADR-0059 is extended**: a second kind of state source (an event stream)
  and two more vendors. The shared `hook status` command prints nothing for
  Codex, whose Stop hook fails on `{}`.
- **ADR-0036 is extended**: the MCP endpoint is a new kind of client, not a
  browser verb. It needs a row in ARCHITECTURE.md §9.
- **ADR-0040 gains a squad checklist** (D19). Squad wiring per vendor lives
  in the daemon's console spec, the Tier 4 place that already holds
  per-vendor launch arguments. The daemon still links no vendor crate
  (ADR-0032 §10).
- A relaunched console starts a new vendor session; the squad's letters
  survive a daemon restart, the agents' memory does not. Resuming by the
  vendor's session id is later work.
- The delivery order is S0 (done), this ADR, then E1–E10 as listed in the
  research record §12a (G16). Inside each step, Claude first, then
  OpenCode, then Codex.

## Considered options

- **Use each vendor's own messaging, and govern it.** Only Claude has one,
  and it reaches every session of the same OS user. Two channels would split
  the fence timeline and hide half of the waits from the stall rule.
- **Build an agent loop in Ralphy, so Ralphy can put input anywhere in it.**
  That is what an agent framework does when it owns the loop. It would
  replace the vendor CLIs, their subscriptions and their tools, which are
  what Ralphy drives.
- **A lead agent that routes every message (a tree).** It makes waits
  between peers impossible by shape, but takes from the operator the free
  team they asked for. A lead is a duty the operator may choose (D11), not a
  rule.
- **Type letters into the receiver's PTY.** Works for every TUI, but the
  receiver reads another agent's words as the operator's, and it collides
  with the operator's own typing.
- **Agent Client Protocol or a vendor's app server as the console.** Gives
  mid-turn control, but replaces the vendor's TUI that the operator watches
  and types into. It can come later, vendor by vendor.
- **Only the daemon computes membership, and the browser always reads it.**
  Most uses in the browser happen during a gesture, on rects the daemon has
  not stored yet; a round trip there would lag the drag or answer with the
  old layout. One rule in two copies, held by one fixture, keeps a single
  meaning without that cost.
- **A stored `fenceId` on each desk record.** A second fact that can
  disagree with the geometry: a console drawn outside a fence could still
  carry an old id.
- **A database (SQLite, libSQL) for letters.** The volume is small, there is
  one writer, and every other daemon store is a file. SQLite through
  `rusqlite` is already in the daemon binary (read-only today, through the
  usage scan), so it stays the upgrade when a real query need appears.
  libSQL's extra value is replicas and sync between server instances, which
  the daemon is not.
- **Squad tools for runs too.** A run's roles already have role artifacts,
  and the Copilot and Cursor adapters turn extra MCP servers off on purpose
  (ADR-0041, ADR-0042).

## Compliance

- D1: not checked by code: not built yet; a planned check that no run argv
  (`crates/ralphy-cli/src/run/`) carries the squad MCP config or the
  console token variable.
- D2: not checked by code: not built yet; the planned check is the shared
  membership fixture, read by a Rust test of the desk and by a test in
  `crates/ralphy-daemon/ui-tests/`, in the pattern of the shared replies of
  ADR-0070. A second planned check: no squad route reads membership from a
  request body.
- D3: not checked by code: the deny rule lives in per-console vendor
  settings; a test of the Claude console spec will pin it.
- D4: not checked by code: not built yet; a test that the doorbell text and
  the hook replies never contain a letter body will pin it.
- D5: not checked by code: the vendor ways in need live CLIs; the doorbell
  text will be one constant.
- D6: not checked by code: not built yet; a planned test that every squad
  route refuses a request without a console token on a loopback bind.
- D7: not checked by code: not built yet; the tool list will be a Rust enum,
  matched exhaustively.
- D8: not checked by code: needs live vendors; the live checklist covers it.
- D9: not checked by code: the fold is pinned by the shared fixture of
  ADR-0059 (`agent_state.rs`), a behaviour test; the event-stream source
  will join the same fixture.
- D10: not checked by code: a behaviour rule of a feature not built yet.
- D11: not checked by code: the desk apply code will refuse a second lead,
  pinned by a behaviour test.
- D12: not checked by code: manual, reviewed in the PR.
- D13: not checked by code: a behaviour rule of a feature not built yet.
- D14: not checked by code: not built yet; a planned test that the tool
  refuses a console whose kind is an agent.
- D15: not checked by code: a behaviour rule of a feature not built yet.
- D16: not checked by code: manual, reviewed in the PR.
- D17: not checked by code: needs a live vendor server; the live checklist
  checks it.
- D18: not checked by code: a documentation duty.
- D19: not checked by code: the ADR-0040 inventory is reviewed by hand.
