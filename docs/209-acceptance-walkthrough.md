# #209 operator acceptance walkthrough — 2026-07-14

Daemon: real, Windows, port 7357 (plus a second real daemon on port 7358 for
the network-bind Session-login half of C3). Produced by
`crates/ralphy-daemon/tests/wb_accept_209.py`, 30/30 checks passed,
`ALL SYMPTOMS NOT REPRODUCIBLE`.

None of the five originally-audited symptoms
(`docs/audit-workbench-2026-07-13.md`) reproduce, plus three extras
corroborating #202-#208.

| # | Audit id | Symptom | Result | Screenshot |
|---|----------|---------|--------|------------|
| 1 | C1 | Demo badge / daemon-mode detection (#202/#208) | PASS | `209-demo-badge-2026-07-14.png` |
| 2 | C2 | Kanban error state distinct from empty (#207) | PASS | `209-board-error-2026-07-14.png` |
| 3 | C3 | Auth honesty — loopback | PASS | `209-auth-honest-2026-07-14.png` |
| 4 | C3 | Auth honesty — network-bind Session TOTP login (#179/#205) | PASS | none (headless, no visual criterion — `/api/session` + gate-hidden assertions) |
| 5 | A2 | Kanban stays above a focused floating console (#208) | PASS | `209-kanban-above-consoles-2026-07-14.png` |
| 6 | A4/A5 | Tree integrity — `.ralphy`/`.github` visible, `.git`/`.env` excluded, reconcile not append (#203) | PASS | `209-tree-integrity-2026-07-14.png` |
| 7 | M8 | Translation errors/decisions are actionable (#206) | PASS | none (pure-function asserts, no live model — Translator API absent headless) |
| 8 | M1 | Topbar uptime is a live heartbeat, not a static string (#204) | PASS | `209-topbar-live-2026-07-14.png` |
| 9 | A6 | Viewer external-edit refresh mechanism (#203) | PASS | none (DOM-badge + directory-nudge reapply asserted programmatically) |

## Detail

- **C1/B1** — `.demo-badge` visible under `file://`; `WBMode.isDemo()` true
  under `file://` and `WBMode.isDaemon()` true under `http://`.
- **C2** — with `boardError['fixture']` set, `.kanban-error` shows the exact
  daemon message (`gh: not authenticated`) and both `.kanban-empty` variants
  stay hidden — error is visually distinct from empty.
- **C3 loopback** — on `policy === 'localhost'`, the Require-login checkbox is
  disabled, the honesty note ("this bind is loopback…") is shown, the TOTP
  enroll button stays usable, and Log off is a no-op (no dead-end login
  screen on a bind that has none).
- **C3 network-bind** — a SECOND real daemon, bound `0.0.0.0:7358` (genuinely
  non-loopback per `AuthPolicy::for_bind`, reached locally over `127.0.0.1` —
  no external network exposure) with a pre-seeded TOTP secret, resolves to
  `AuthPolicy::Session`. The login-gate blocks the shell pre-auth; a stdlib
  `hmac`/`hashlib`/`struct` RFC 6238 TOTP computed from the same seed, typed
  into the real login form and submitted, authorizes: the gate hides, Alpine's
  `authed` flips true, and a follow-up `GET /api/session` independently
  confirms `authed:true, policy:"session"`. Deviates from the plan's literal
  "enroll via `POST /api/security/totp/enroll`" step — the seed is pre-written
  to disk instead, because `AuthPolicy` is resolved once at daemon boot
  (`lib.rs` `serve()`): an enroll call *after* boot only writes the seed file,
  it cannot promote an already-running `Bearer` policy to `Session`. Login
  itself is driven through the real browser form, not a raw HTTP POST — more
  faithful to an operator's path, same server-side code exercised.
- **A2** — a focused floating console's z-index climbs above the board's, yet
  `elementFromPoint` at its centre still resolves inside `.kanban`, never
  `.session-window` (`.stage`'s `isolation: isolate` traps the stacking
  context).
- **A4/A5** — a throwaway `git init` fixture repo (never the live
  `C:\Dev\ralphy` tree) with a committed `.ralphy/plan.md`, `.github/x.yml`,
  and a gitignored `.env`. `tree.list` on the root includes `.ralphy` and
  `.github`, excludes `.git` and `.env`. Two sequential `file.write` calls to
  the same directory followed by a re-`tree.list` each report the identical
  child count (5) — reconcile, not append.
- **M8** — `WBTranslate.explainError`, `.decide`, and `.progressText` (pure,
  DOM-free functions extracted in #206) assert their exact mapped strings; no
  live Translator-API screenshot since headless chromium 1.60 lacks it.
- **M1** — after the real `/ws` presence heartbeat ticks at least once,
  `uptimeText` is non-empty and not the `"connecting…"` placeholder; the
  topbar's rendered text starts with `up `.
- **A6** — the `.viewer-disk-badge` element is present in the viewer DOM
  (hidden on a clean tab); after a real `file.write` to the open file and a
  simulated directory nudge (`refreshOpenViewers('')`, what a real
  `tree.dirty` push drives), the clean tab's rendered body picks up the new
  bytes without user action. Deeper dirty-vs-clean badge UX (the "changed on
  disk" banner path) is `[review-only]` — flagged below, not exercised by this
  pass.

## Review-only (human judgment)

- The operator's own end-to-end pass through this record, confirming it
  matches what they see when running the daemon by hand (AC#1 — a human
  operating a real Windows daemon; the script above mechanizes the roteiro,
  this is a thin confirmation, not a re-derivation).
- The dirty-tab "changed on disk — reload" banner UX (badge shown, click
  applies `pendingDisk`, discarding an unsaved edit is the RIGHT call) is not
  exercised here — only the clean-tab auto-refresh path is. Cheapest close: a
  manual pass — edit a file in the UI without saving, then externally modify
  it on disk, and confirm the badge appears and Reload applies the disk
  version.
- The live on-device translation download/progress UX (Translator API model
  download, real progress ticks) is unverifiable headless (Chrome/Edge 138+
  only) — the pure-function mapping is `[verified]`, the live UX is not.

## Divergences

None found. All five originally-audited symptoms (C1-C3, A2, A4/A5, M8) plus
the three extras (M1, A6, and the C3 network-bind hard half) are confirmed
NOT reproducible against the real, post-remediation daemon.
