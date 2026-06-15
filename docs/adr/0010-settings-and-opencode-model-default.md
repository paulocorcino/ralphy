# Ralphy settings file + persistent OpenCode model default (amends ADR-0005 D4)

Status: accepted — amends ADR-0005 D4.

Introduce a per-repo `.ralphy/settings.json` as the foundation for operator
configuration, and use its first key to persist an OpenCode execution-model
default. This lets the operator pick a model once (`kimi-for-coding/k2p7`)
instead of retyping `--exec-model` every run, while keeping a single
deterministic resolution order.

## Decision

The OpenCode execution model resolves in this precedence:

```
--exec-model X            (per-run flag)            → strongest
settings.json opencode.model (persistent default)   → middle
omit -m                   (OpenCode's own resolution)→ weakest
```

This **amends ADR-0005 D4** ("omit `-m` unless `--exec-model` is set, deferring
to OpenCode"): a persisted `opencode.model` now *also* yields a concrete `-m`.
An unset or empty setting falls cleanly back to OpenCode's own resolution
(`opencode.json` `model` → last-used → priority), so "use the OpenCode default"
stays a first-class, selectable state and remains the out-of-the-box default.

Operator surface (a config subcommand, consistent with `ralphy telegram`):

```
ralphy models --agent opencode               # passthrough to `opencode models`
ralphy config set opencode.model kimi-for-coding/k2p7
ralphy config unset opencode.model           # back to OpenCode's default
ralphy config get                            # show current settings.json
```

`settings.json` lives under the gitignored `.ralphy/` (per-repo) and its schema
tolerates unknown keys, so future configuration grows in the same file without a
migration.

## Why (and why D4 was right until now)

D4 deferred entirely to OpenCode to avoid duplicating its resolution and to keep
one source of truth (the operator's `opencode.json`). The operator now wants a
ralphy-owned persistent default. The amendment preserves D4's anti-ambiguity
goal: the setting, **when present**, is passed verbatim as `-m`, which is also
the strongest input to OpenCode's own resolution — so there is no silent
two-source conflict, only an explicit operator override that an empty setting
removes. We did **not** re-implement OpenCode's config parsing or model
discovery (the listing is a passthrough to `opencode models`).

## Consequences

- Listing is **OpenCode-only** for now — it is the only vendor with a native
  model lister and an open, operator-owned model space (Claude's are documented
  `sonnet`/`opus`; Codex resolves from `config.toml`).
- The OpenCode adapter's resolved-model log is brought to parity with Claude and
  Codex (which already log the resolved model): OpenCode logs `usage.model`
  (read from `opencode.db`, ADR-0008 D5) at end of `plan`/`execute`, so the
  operator can confirm what actually ran without reading the ledger.
- The Telegram config stays its own global TOML file for now; folding it into a
  global `settings.json` is a later, separate decision.
