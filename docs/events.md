# Ralphy event contract (CloudEvents)

The living reference for the CloudEvents stream Ralphy emits to a configured
HTTP endpoint. Decisions and philosophy live in
[ADR-0019](./adr/0019-cloudevents-event-sink.md) (the sink) and
[ADR-0020](./adr/0020-issues-query-surface.md) (`ralphy issues` and the queue
snapshot); this document records the shapes a consumer programs against.

Status: contract for the first implementation — field lists here are the
target; the source of truth once implemented is the sink code plus its
per-event tests.

## Transport

- `POST {events.url}` with `Content-Type: application/cloudevents+json`
  (CloudEvents 1.0, structured mode), one event per request. URL and token
  are configured per repo in the global store (`~/.ralphy/events.toml`, via
  `ralphy config set events.url` / `events.token`); the
  `RALPHY_EVENTS_TOKEN` env var overrides the stored token for a run.
- `Authorization: Bearer <token>` when a token is configured. The token
  authenticates the **emitter**; `data.emitter.user` is self-declared
  (`git config`) and is attribution, not authentication — a platform wanting
  per-person trust should issue per-dev tokens and map token→person.
- **Endpoint contract**: any `2xx` acknowledges the event (body ignored).
  `5xx`, timeouts and network errors are transient — retried ~3 times with
  short backoff, then dropped. Any `4xx` is treated as a configuration error
  and dropped without retry.
- Delivery is **at-most-once**: bounded queue, short retry, then drop.
  Consumers must dedup by `id` and infer liveness from `run.heartbeat`,
  not from stream completeness.
- **Ordering**: a single sender task per process sends in emission order, so
  per-`runid` order is preserved in practice, but drops leave gaps. Order by
  `id` (ULIDs are millisecond-sortable) rather than by `time`, whose second
  precision collides.

## Envelope

```json
{
  "specversion": "1.0",
  "type": "dev.ralphy.issue.closed",
  "source": "ralphy/<owner>/<repo>",
  "subject": "issue/89",
  "id": "01JZ7Q8R9S…",
  "time": "2026-07-03T17:22:31Z",
  "runid": "01JZ6XK4M2…",
  "datacontenttype": "application/json",
  "data": {
    "emitter": {
      "version": "0.1.0-rc10",
      "user": "paulo@corcino.com.br",
      "host": "PICHAU",
      "os": "windows-11",
      "pid": 18432,
      "ip": "203.0.113.7",
      "tz": "America/Sao_Paulo"
    },
    "git": { "repository": "o/r", "branch": "afk/run-20260703-172231" },
    "issue": { "number": 89, "title": "normalize the envelope data" },
    "agent": { "name": "claude", "model": "claude-sonnet-4", "effort": "medium" }
  }
}
```

Core attributes: `type` is namespaced `dev.ralphy.<noun>.<event>`; `source`
identifies the repo the run works; `subject` is `issue/<n>` on every
`dev.ralphy.issue.*` event and on `plan.written` / `plan.step` / `plan.opened`
/ `plan.closed`, absent on run-scoped events; `id` is a per-event ULID (the
dedup key and sort key); `time` is always UTC (RFC 3339).

Vocabulary for external readers: **green** = an issue whose execution
finished cleanly, which the runner then closes; **non-green** = any other
stop (stuck, blocked, timeout). Token counters come in the ledger's four
fields — `up` input, `cr` cache-read, `cw` cache-write, `out` output.

### Emitter identity (on every event)

CloudEvents extension attributes must be simple types (the spec forbids
nested values at the envelope level), so the envelope carries exactly **one**
extension — the correlation key — and the rest of the identity groups under a
reserved `emitter` object inside `data`, keeping the header clean:

| Field | Meaning | Role |
| --- | --- | --- |
| `runid` (envelope extension) | ULID minted at process start | **Primary key** — groups a run's events across the fleet; filter/route on it without parsing `data` |
| `data.emitter.version` | Ralphy binary version | Which contract vintage is emitting |
| `data.emitter.user` | `git config user.email` | Attribution to a person |
| `data.emitter.host` | Hostname | Which machine |
| `data.emitter.os` | e.g. `windows-11`, `linux`, `macos` | Per-OS diagnostics |
| `data.emitter.pid` | Process id | Which process among concurrent Ralphys on one host |
| `data.emitter.ip` | **Public egress IP** (best-effort): probed at run start via `checkip.amazonaws.com` → `checkip.global.api.aws` → `icanhazip.com` → Cloudflare `cdn-cgi/trace`, each ~2s; falls back to the primary LAN IP, then `0.0.0.0`, when every probe fails | Network diagnostic — never a key (multi-NIC, DHCP, VPN) |
| `data.emitter.tz` | Local timezone: IANA name (`America/Sao_Paulo`) when resolvable, else fixed offset (`-03:00`) — parsers accept both | Reconstruct local time from UTC `time` |

`emitter` is a reserved key on every event's `data`, alongside the
event-specific fields listed in the catalog below.

### Reserved `data` blocks (git / issue / agent)

Three more reserved objects ride inside `data` (never as envelope extensions,
so `runid` stays the only one), giving a consumer the run's git, issue, and
agent context without folding the whole stream:

| Block | Present on | Shape | Notes |
| --- | --- | --- | --- |
| `data.git` | **every** event | `{repository, branch}` | `repository` is the `owner/repo` slug (a consumable duplicate of the routing `source`); `branch` is the operating run branch commits land on (`afk/run-<stamp>` in `new` mode, the current branch in `current` mode). Constant per run. |
| `data.issue` | **subject-scoped** events (`issue.*`, `plan.*`) | `{number, title}` | Absent on run-scoped events (`queue.*`, `run.*`). `title` resolves from run state, falling back to the `queue.built` seed, then empty. |
| `data.agent` | **every** event **except `run.finished`** | `{name, model, effort}` | `name` is the current phase's agent — the plan agent while planning, the exec agent while executing, falling back to the run's exec agent before a phase begins, or `null` before `run.started` is seen. `model`/`effort` are `null` before a phase begins. A single triplet — the run's `plan_agent` scalar stays on `run.started`. |

## Event catalog

Types mirror the canonical `RunEvent` decoder
([runstate.rs](../crates/ralphy-cli/src/runstate.rs)) plus three emissions
introduced by ADR-0019 (`run.started`, `run.finished`, `run.heartbeat`), one
by ADR-0020 (`queue.snapshot`), and three raw/step plan events added by the
#96 normalization (`plan.step`, `plan.opened`, `plan.closed`). Token
breakdowns use the ledger's four counters: `up` input, `cr` cache-read, `cw`
cache-write, `out` output (ADR-0008).

| `type` (`dev.ralphy.` prefix) | When | `data` fields |
| --- | --- | --- |
| `run.started` | Process begins working a queue | `repo`, `queue_labels[]`, `plan_agent`, `branch_mode`, `base` (the base branch — renamed from `branch`), `deadline_hours?`, `queue[]` — a light `{number, title}` scope list seeded from the preceding `queue.built` (the exec agent is now `data.agent.name`) |
| `queue.built` | Queue resolved from labels | `count`, `order[]` (issue numbers), `stop_before?`; **enriched (ADR-0020)** with `issues[]` — per-issue `{number, title, labels[], queue_status, skip_reason?, blocked_by[], position?}` (`position` only on `eligible` issues) |
| `issue.started` | Work begins on an issue | `number`, `title` |
| `issue.planning` | Planning phase starts | `model?`, `effort?` |
| `plan.written` | Plan artifact written | `number`, `open_steps` (`0` = infeasible), `usage {up,cr,cw,out,model}`, `steps[]` — the full checkbox list as `{text, status}` (`status`: `open` \| `checked` \| `noticed`) |
| `plan.step` | A plan checkbox transitioned (once per transition) | `text` (the normalized step text — the step identity), `status` (`checked` \| `noticed`) |
| `plan.opened` | Raw plan snapshot at the plan-write point | `number`, `plan_md` (the complete raw `plan.md`) |
| `plan.closed` | Raw plan snapshot at the issue close (before the next `plan()` overwrites it) | `number`, `plan_md` (the complete raw `plan.md`) |
| `issue.executing` | Execution phase starts | `number`, `budget_min`, `model`, `effort?` |
| `issue.closed` | Green — the cycle closes the issue | `number`, `tokens` (flat total across **plan + execute**), `usage {up,cr,cw,out,model}` (execution phase only) |
| `issue.non_green` | Non-green stop (stops the run) | `number`, `outcome` — the core's outcome name (e.g. `Stuck`, `Blocked`, `Timeout`); treat as an **opaque display label**, not an enum to switch on |
| `issue.needs_split` | Planner judged the issue a bundle | `number` |
| `issue.skipped` | Issue skipped, run continues | `number`, `kind` (`blocked_by` \| `stop_before` \| `human_return` \| `verify_failed`), `label?` (parking label on `human_return`, ADR-0016) |
| `issue.human_blocked` | Human gate in the dependency path (ADR-0014) | `number`, `on[]` (issues a person must clear) |
| `issue.deadline_passed` | Deadline hit before the issue started | `number` |
| `run.sleep_started` | Usage-limit sleep begins (ADR-0003) | `reset`, `target_epoch` |
| `run.sleep_ended` | Reset reached, run resumes | — |
| `knowledge.consolidating` | End-of-run consolidation starts | `notes` |
| `knowledge.consolidated` | Consolidation finished | `archived` |
| `run.notice` | Any WARN/ERROR on the bus | `level`, `message` |
| `run.heartbeat` | Every ~30s while the process lives — **including during usage-limit sleeps** (`phase: "sleeping"`), so a long sleep is never mistaken for death | see below |
| `run.finished` | Clean end of a run (never on crash/kill — detect those by heartbeat silence) | `outcome` (`completed` \| `non_green` \| `deadline` \| `stop_before`; only `completed` means the whole queue was worked), `issues_done`, `issues_skipped`, `issues_total`, `issues[]` — per-issue rollup `{number, title, status, kind?}` (`status`: `done` \| `skipped` \| `blocked` \| `infeasible` \| `needs_split` \| `non_green` \| `hitl`; `kind` only on a `skipped`); `tokens_total {up,cr,cw,out}`, `duration_s`. Carries **no** `data.agent` block. The `issues[]` rollup lists only issues that **entered the run**; key completeness off the scalar `issues_total`, not the array length |
| `queue.snapshot` | On demand: `ralphy issues --push` (ADR-0020) | identical `data` shape to the enriched `queue.built` (`count`, `order[]`, `stop_before?`, `issues[]`) |

### `run.heartbeat`

A compact `RunState` summary so a consumer renders "now" without a perfect
fold, and declares a run dead by silence (recommended threshold: ~3 missed
beats). `interval_s` carries the emitter's own cadence so the consumer never
hardcodes it. `phase` is one of `starting | planning | executing | sleeping |
consolidating`. `issue` is the active issue as `{number, title}`, or `null`
when none is active (a normalized shape — it was a bare number in the first
contract vintage):

```json
{
  "phase": "executing",
  "interval_s": 30,
  "issue": { "number": 89, "title": "normalize the envelope data" },
  "elapsed_s": 412,
  "queue_done": 2,
  "queue_total": 7,
  "tokens_total": { "up": 91200, "cr": 448000, "cw": 60100, "out": 15400 }
}
```

## Consumer guidance

- **Key by `runid`**, fold events into per-run state exactly as the Telegram
  notifier folds `RunEvent` (ADR-0007) — order by `time`, dedup by `id`.
- **Attribute** by `data.emitter.user`; **locate** by `data.emitter.host` +
  `data.emitter.pid`.
- **Tolerate absence**: at-most-once delivery means any discrete event can be
  missing; heartbeats and terminal events (`issue.closed`, `run.finished`)
  carry enough totals to converge.
- **Ignore unknown fields and unknown types**: that is how this contract
  grows.

## Evolution rules

1. `data` payloads evolve **additively only** — new optional fields are the
   normal path; removing or renaming a field, or changing a field's meaning,
   requires a **new event type** instead.
2. New event types may appear at any release; consumers skip unknown types.
3. `data.emitter.version` on every event identifies the emitting contract vintage
   when forensics are needed.
4. `runid` and the `data.emitter` block are stable; new emitter fields or
   envelope extensions may be added, never repurposed.
5. A future finer-grained level (agent tool-calls, ADR-0019 §6) will arrive
   as new `dev.ralphy.agent.*` types behind a settings knob — never as extra
   fields on existing types.
6. This document — the prose tables — **is** the contract for now; a
   machine-readable JSON Schema is deliberately deferred until the shapes
   stabilize against a real consumer.
