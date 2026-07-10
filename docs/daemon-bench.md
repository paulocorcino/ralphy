# Daemon bench guide: hands-on testing for Phase 1

The operator's manual for bench-testing `ralphy daemon` (ADR-0032, PRD #157),
structured around the seven acceptance scenarios of issue **#169**. Every
command and path below is the implemented surface, not the plan.

## The surface at a glance

| Thing | Value |
|---|---|
| Start (foreground) | `ralphy daemon` — Ctrl+C stops it |
| Default listener | `http://127.0.0.1:7257` ("ralphy" on a keypad) |
| Flags | `--port <p>`, `--bind <ip>` |
| Baptism | `ralphy daemon setup` (name + avatar + token mint) |
| Status | `ralphy daemon status` (identity, token, listener, autostart) |
| Registry | `ralphy daemon add <path>` / `remove <owner/repo>` |
| Autostart | `ralphy daemon install` / `uninstall` |
| Global store | `~/.ralphy/daemon.toml` (identity), `~/.ralphy/repos.toml` (registry), `~/.ralphy/daemon-token` (access token) |
| Token env override | `RALPHY_DAEMON_TOKEN` (stripped from every child the daemon spawns) |
| Test isolation | `RALPHY_DAEMON_DIR=<dir>` points all three stores at a scratch dir |

Command verbs the UI dispatches (`/ws/command`) map to exactly the blessed
invocations — nothing else is reachable:

| UI action | Spawned child |
|---|---|
| run | `ralphy run --if-idle` |
| triage | `ralphy triage --if-idle --yes` |
| push | `ralphy issues --push` |

There is deliberately **no kill verb**: a dispatched run always outlives the
browser and the daemon itself.

## Build

```
cargo build --release
```

Use `target/release/ralphy` below (or `cargo run --` while iterating). For a
disposable bench that never touches your real `~/.ralphy`:

```
RALPHY_DAEMON_DIR=/tmp/ralphy-bench ralphy daemon setup
RALPHY_DAEMON_DIR=/tmp/ralphy-bench ralphy daemon
```

(Set the same variable on every `ralphy` invocation in that bench, including
the runs you trigger, or their passive registration writes to the real store.)

---

## Scenario 1 — install, baptism, autostart

1. `ralphy daemon setup`
   - Name prompt suggests a hostname-derived default; Enter accepts it.
   - Try a reserved name first (`run`, `queue`, `forge`…): it must be refused
     with a message naming the collision, then re-prompt.
   - Pick an avatar **by number** from the list.
   - The **access token is printed exactly once** — copy it now (Scenario 4).
     A re-run of `setup` re-baptizes (new name/avatar, same `daemon_id`) but
     never re-shows the token.
2. `ralphy daemon status` → expect `🐙 <name>`, `access token: set`,
   `listener: http://127.0.0.1:7257`, `autostart: not registered`.
3. `ralphy daemon install` → registers a logon Task Scheduler task named
   `ralphy-daemon` (Windows) or a systemd user unit `ralphy-daemon.service`
   (Linux/WSL). `status` now says `autostart: registered`.
4. Reboot (or log off/on). The daemon must be listening without any manual
   start: open `http://127.0.0.1:7257` and see the page with your avatar+name.
5. `ralphy daemon uninstall` and confirm `status` flips back; re-`install`
   for the rest of the bench. Both commands are idempotent — run each twice.

**Pass:** identity survives restarts (`daemon_id` in `daemon.toml` never
changes); reserved names refused; token shown once; autostart round-trips.

## Scenario 2 — passive repo registry

1. With the daemon running, go to any repo the daemon has never seen and run
   a cheap repo-scoped command (e.g. `ralphy issues`).
2. Refresh `http://127.0.0.1:7257` → the repo appears in the list (slug from
   the `origin` remote), reachable.
3. **Move the repo** to another directory. The UI still lists the old path
   (entries never self-delete) — after the next `ralphy issues` from the new
   location, the same slug shows the new path: self-healed, no duplicate.
4. Rename/remove the directory entirely → entry shows **unreachable**
   (greyed), still listed.
5. `ralphy daemon remove <owner/repo>` deletes it; `ralphy daemon add <path>`
   registers one that never ran. Repeat both — idempotent.

**Pass:** registration is a side effect of normal use; a moved repo heals by
slug; removal is only ever manual.

## Scenario 3 — workbench session at the desk

1. On the page, click a repo tile → agent **claude** (or codex/opencode).
2. A live terminal opens (xterm.js) running the real CLI **in the repo's
   directory**. Type — keystrokes echo through the real PTY.
3. Resize the browser window: the CLI's TUI must reflow (rows/cols propagate
   to the PTY; no wrapped garbage).
4. The sessions panel lists it (`repo`, `agent`, kind `agent`, started-at).
5. Close it from the panel (`POST /api/sessions/close`): the child process
   tree must be gone (check Task Manager / `ps`).

**Pass:** zero-typing launch, correct cwd, resize works, close kills the tree.

## Scenario 4 — phone over Tailscale (bind + token + reattach)

1. Stop the daemon. Restart bound to the Tailscale interface:
   `ralphy daemon --bind <tailscale-ip>`.
   - **Guard check:** on a machine with no token minted, this must **refuse
     to start** (a non-loopback bind without a token aborts at boot).
2. From the phone (Tailscale on), open `http://<tailscale-ip>:7257`:
   - Without the token → **401** on everything, including the page.
   - With the token (the UI prompts / `Authorization: Bearer <token>`) → the
     page loads and works. Localhost on the desktop still needs no token.
3. Open a session from the phone. Turn on **airplane mode** for ~10s, back
   on, reopen the page → the session is still in the panel; **reattach**
   replays the scrollback backlog, then streams live.
4. Reattach from the desktop while the phone holds it: expect a **busy**
   refusal (409) unless you confirm the **takeover** prompt, which evicts the
   phone cleanly (single-writer rule).
5. Hygiene check: in a free console spawned by the daemon, print the
   environment — `RALPHY_DAEMON_TOKEN` must **not** be present.

**Pass:** no-token network bind refuses to boot; 401 without bearer;
localhost exempt; drop → reattach with backlog; takeover explicit; token
never reaches children.

## Scenario 5 — the morning/afternoon session

1. Morning, desktop: launch a Claude session, give it a task that ends in a
   question back to you. Walk away — close the browser tab entirely.
2. Afternoon, phone: open the page → the session is still listed (the child
   never died with the tab). Reattach, read the scrollback (the question),
   answer it, watch it continue. Same session, same PTY.

**Pass:** the session belongs to the daemon, not the connection.

## Scenario 6 — remote run trigger + daemon identity in events

Prereq: a repo with one issue carrying the queue label, and (for the events
check) an `events.url` configured (ADR-0019).

1. From the phone/desktop UI, hit **run** on the repo. Expect two frames in
   the UI: `spawned` (with pid), then later `exited` (with code). The run
   behaves exactly like a cron-fired `ralphy run --if-idle`.
2. Hit **run** twice quickly: one run proceeds; the second child exits 0
   logging `skipped: run in progress` (the repo-scoped presence lock).
3. Events check: the dispatched run's CloudEvents carry
   `data.emitter.daemon_id` = your daemon's ULID. Then run `ralphy run`
   **by hand** in the same repo (or from a free console): its events carry
   **no** `daemon_id` — truthful absence.
4. Kill test (the invariant, not a button): while a dispatched run is mid
   flight, Ctrl+C the daemon. The run **must keep going** to completion; the
   daemon merely stops serving. Restart the daemon afterwards.

**Pass:** blessed invocation only, overlap absorbed, `emitter.daemon_id`
present iff daemon-spawned, runs survive the daemon.

## Scenario 7 — the WSL daemon

WSL is a plain Linux fleet member: its **own** daemon, `~/.ralphy`, port.

1. Inside the distro: build/install the Linux `ralphy`, `ralphy daemon
   setup` (different name!), `ralphy daemon install` (systemd user unit —
   needs systemd enabled in `/etc/wsl.conf`).
2. Start it on a **different port** if you'll run both daemons at once:
   `ralphy daemon --port 7258` (two listeners, one machine).
3. Repeat a session (Scenario 3) and a trigger (Scenario 6) against a repo
   that lives in the WSL filesystem.
4. Wake-at-logon: the distro must be up for its daemon to serve. The
   documented nudge is a Windows logon scheduled task running
   `wsl -d <distro> true`. Verify after a reboot that the WSL daemon answers.

**Pass:** identical behavior, zero cross-boundary hacks, both daemons
distinguishable by name/avatar.

---

## Troubleshooting

- **Page loads but no identity shown** — daemon not baptized: `ralphy daemon
  setup`, restart the daemon (identity is loaded at boot).
- **`daemon --bind <ip>` exits immediately** — that's the guard: mint a token
  (`setup`) or set `RALPHY_DAEMON_TOKEN`.
- **Repo missing from the list** — the registry is read fresh per request,
  so it's a write-side issue: the command in that repo predates the branch,
  or `RALPHY_DAEMON_DIR` split your stores.
- **Reattach says 404** — the session ended (child exited or was closed);
  sessions don't survive a daemon restart (they're process state, by design).
- **Reattach says 409** — another client is attached; retry with takeover.
- **Presence stops ticking** — heartbeat is 2s over `/ws`; a dead daemon is
  detected by silence, not by an error frame.
- Foreground log verbosity: `RALPHY_LOG`/`RUST_LOG` (e.g. `RUST_LOG=debug`).

## Reporting

Work through the scenarios in order (each builds on the previous). Per #169:
friction becomes follow-up issues, never silent local patches; when all seven
pass, sign off on #169 and the PRD's Phase-1 scope is delivered.
