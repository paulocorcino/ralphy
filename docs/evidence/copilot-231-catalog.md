# Evidence — #231: the Copilot preflight learns the catalog for free

Host: Windows 11 (10.0.26200), `GitHub Copilot CLI 1.0.71`, 2026-07-20.

## The probe

```
env -u GH_TOKEN -u GITHUB_TOKEN -u COPILOT_GITHUB_TOKEN \
  copilot -p "hi" --model "zzz-not-real" --allow-all-tools \
  --no-remote --no-remote-export --disable-builtin-mcps \
  --no-auto-update --no-ask-user --log-level all --log-dir <tmp>
```

Observed exit code: **1** (`Error: Model "zzz-not-real" from --model flag is not
available.` on stderr, nothing on stdout). The planning pass recorded `0` for the
same command on the same host and CLI version, and the ADR-0041 spike §4b recorded
`1` — which is exactly why `fetch_catalog` never inspects the exit status and keys
only on the CAPI log line.

Log written: one `process-1784553062237-11976.log`, 64 777 bytes.

## What the log carried

- `[rust:capi_models] fetched models from CAPI /models {...}` — one line, 54 933
  bytes. `models` is a JSON **string** holding the array.
- `2026-07-20T13:11:10.836Z [INFO] Using default model: claude-sonnet-5`
- `2026-07-20T13:11:10.836Z [WARNING] Model 'zzz-not-real' from CLI argument is not
  available. Falling back to next option.`

Counts: **46** entries, **15** with `model_picker_enabled` (selectable).

Those three lines, verbatim, are the committed fixture
`crates/ralphy-agent-copilot/fixtures/capi-models-2026-07-20.log`; the rest of the
log (local paths, MCP URLs, session ids) was discarded.

## The free-ness oracle

`cargo test -p ralphy-agent-copilot -- --ignored --exact catalog::tests::live_probe_fetches_the_catalog_for_free`

```
running 1 test
test catalog::tests::live_probe_fetches_the_catalog_for_free ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 35 filtered out; finished in 7.11s
```

The test asserts `copilot_usage(&cat.probe_session_id) == Usage::default()` — the
session id the probe itself minted wrote **no** `assistant_usage_events` row.

Vacuity check (temporary edit, reverted): pointing the same assertion at a session
id known to have rows REDS —

```
left: Usage { input: 5519473, output: 42503, cache_read: 5355709, cache_creation: 146563, model: Some("claude-sonnet-5") }
right: Usage { input: 0, output: 0, cache_read: 0, cache_creation: 0, model: None }
```

## Rate card / effort samples pinned from the fixture

| id | selectable | restricted_to | reasoning_effort | in / out / cache_read / cache_write | max_prompt_tokens |
|---|---|---|---|---|---|
| `claude-sonnet-5` | yes | pro, pro_plus, business, enterprise, max | low, medium, high, xhigh, max | 200 / 1000 / 20 / 250 | 200000 |
| `gpt-5-mini` | yes | *(absent → every tier)* | low, medium, high | 25 / 200 / 2 / 0 | *(absent)* |
| `claude-opus-4.8` | no | pro_plus, business, enterprise, max | low, medium, high, xhigh, max | 500 / 2500 / 50 / 625 | 200000 |
| `kimi-k2.7-code` | yes | pro, pro_plus, individual_trial, edu, max, business, enterprise | *(absent → none)* | 95 / 400 / 19 / 0 | 224000 |

Prices are nano-AIU per 1M tokens. Copilot bills in **AI credits** with an
independent per-model request multiplier, so this card is exposed, never spent —
no `PriceTable` wiring in this slice.
