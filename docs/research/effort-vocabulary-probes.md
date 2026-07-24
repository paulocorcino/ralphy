# Effort vocabulary — cross-vendor probes

Evidence for [#227](https://github.com/paulocorcino/ralphy/issues/227) and
[ADR-0044](../adr/0044-effort-as-ralphy-vocabulary.md). This records **what each
vendor executable actually accepts as a reasoning-effort value**, and how it
behaves on an out-of-range one. Observations only; the decisions live in the ADR.

Session date: 2026-07-22, on **Windows 11 Pro 26200**. Every claim cites the
command run and its output; where a level was asserted only by `--help`, a spike,
or an ADR rather than exercised here, it is marked accordingly.

---

## 0. Why this exists

Effort is not a Ralphy concept today — it is an opaque string each adapter treats
differently, and one vendor's out-of-range fallback *inverts* the operator's
intent silently (ADR-0041 D5a, Copilot probe P6). Before promoting effort to a
Ralphy word we need the real per-executable vocabulary, not the issue's
second-hand summary. This probe found the summary was **incomplete on two
adapters** (Cursor and Gemini *do* have an effort axis) and **wrong about who
degrades silently** (Claude does it too, not only Copilot).

---

## 1. The map

| Executable | Effort lives in | Real vocabulary | Out-of-range value → | Provenance |
|---|---|---|---|---|
| **claude** | flag `--effort <level>` | `low, medium, high, xhigh, max` (5) | ⚠ **warning + default, exit 0** (silent degrade) | live probe (§2) |
| **codex** | `-c model_reasoning_effort=<v>` | `none, minimal, low, medium, high, xhigh, max` (7) | ✅ **API 400, loud** (no degrade) | live probe (§3) |
| **copilot** | flag `--effort <level>` | `none, minimal, low, medium, high, xhigh, max` (7), **per-model** support list | ⚠ valid-but-unsupported → model default, silent (P6) | spike §4 |
| **cursor-agent** | **inside the model id** | suffix `<family>[-thinking]-<none\|low\|medium\|high\|xhigh\|max>[-fast]`; bracket `[effort=high]` | invalid id → loud error | suffix ✅ live (`--list-models`); bracket ✅ help-documented; **free per-model catalog** |
| **gemini** | **settings.json, numeric** | `thinkingConfig.thinkingBudget` — a token budget, not a level | n/a | 📖 doc-only |
| **opencode** | flag `--variant <v>` | **provider-specific**: Anthropic `high\|max`; OpenAI `none…xhigh`; `kimi-for-coding` none | provider rejects | ADR-0005 D3 |
| **kimi** | — | no effort axis | n/a | confirmed (`let _ = effort`) |

### The cross-vendor intersection

Among the four vendors with a **level** axis (Claude, Codex, Copilot, Cursor) the
intersection is exactly **`low, medium, high, xhigh, max`**. Claude is the binding
constraint (it publishes neither `none` nor `minimal`); Cursor lacks `minimal`.
So `none`/`minimal` are the only two rungs not shared by every level-vendor.

`low`/`medium`/`high` are the universal core; `xhigh`/`max` are shared by all four
level-vendors but **not** by every *model within* a vendor (Copilot's per-model
catalog is the proof). Vocabulary breadth (cross-vendor) and model support
(within a vendor) are independent axes.

### The failure-mode split

The out-of-range column partitions the vendors, and this is load-bearing for the
ADR's translation strategy:

- **Silent degraders** — Claude, Copilot. An unsupported value is dropped to a
  default, exit 0, nothing in the stream says so. Normalization in the adapter is
  *mandatory* here or the operator asks for more and silently gets less.
- **Loud rejecters** — Codex (API 400), Cursor (invalid-id error). The vendor
  shouts, so a validated passthrough suffices.

---

## 2. Claude — live

```
$ claude -p "reply OK" --effort zzinvalid
Warning: Unknown --effort value 'zzinvalid' — ignoring it and using the default
effort. Valid values: low, medium, high, xhigh, max.
OK
exit = 0
```

`--effort <level>` is a real flag ("Effort level for the current session" in
`claude --help`). **An unknown value is not rejected — it is ignored with a
warning and the run proceeds at the default, exit 0.** This is the same
intent-destroying shape ADR-0041 attributed to Copilot; it is not Copilot-only.

A follow-up probe, `claude -p "reply with exactly OK" --model haiku --effort
xhigh`, returned `OK`, exit 0, **no warning** — so Claude accepts a valid level on
any model without a per-model signal. Claude publishes **no per-model effort
catalog**: unlike Copilot's CAPI list or Cursor's `--list-models`, there is
nothing to clamp against. So whether the backend silently caps `xhigh` on a model
that tops out lower is *unobservable from the CLI and uncorrectable* — the value-
enum validation is the only guard Ralphy can build. This is a bounded answer, not
an open question: Claude stays *Direct map* by necessity, not by choice.

## 3. Codex — live

```
$ codex exec -c model_reasoning_effort=zzinvalid "reply OK and nothing else"
model: gpt-5.6-sol
reasoning effort: zzinvalid
ERROR: {"type":"error","error":{"type":"invalid_request_error",
 "message":"[ReasoningEffortParam] [reasoning.effort] [invalid_enum_value]
  Invalid value: 'zzinvalid'. Supported values are: 'none', 'minimal', 'low',
  'medium', 'high', 'xhigh', and 'max'."},"status":400}
```

Codex does **not** validate the effort at startup — it forwards the string to the
API, which rejects it with a **400** listing the supported set (the seven OpenAI
`reasoning.effort` values). The error is loud and no completion is billed. This
corrects the issue's "codex hardcodes medium and never exposes the flag": the
axis is fully live, it is only that Ralphy pins `DEFAULT_CODEX_EFFORT = "medium"`
and never threads the operator's value into it (see ADR-0004 amendment, which
already intends effort as "a single global operator override").

## 4. Copilot — from the spike

`--effort` accepts `none | minimal | low | medium | high | xhigh | max`, with a
**per-model** support list published in the CAPI catalog. An out-of-range level
is silently coerced to the model default in both directions (probe P6). Full
table and per-model lists: [copilot-cli-adapter-spike.md §4](copilot-cli-adapter-spike.md).

## 5. Cursor — live via WSL

`cursor-agent 2026.07.20` in WSL Ubuntu, logged in. Reasoning effort is **inside
the model id**, not a flag, and both forms are now confirmed:

- **Suffix**, live via `cursor-agent --list-models`: `-low`, `-medium`, `-high`,
  `-xhigh` appear in the id (`gpt-5.3-codex-xhigh`, `claude-opus-4-8-medium`,
  `gpt-5.6-sol-high`, …). `max`/`none` are in the id-stripping table but did not
  appear in this account's list.
- **Bracket**, documented verbatim in `--help`: *"Parameterized models accept
  quoted bracket `claude-opus-4-8[context=1m,effort=high,fast=false]`"*. No longer
  unverified.

**New finding — Cursor has a free per-model effort catalog.** `--list-models`
enumerates which effort levels each model exposes, and it varies by model
(Composer 2.5 has none; Grok has low/medium/high; Codex 5.3 has low/high/xhigh;
Sol has high/xhigh). This is the same free-enumeration shape as Copilot's CAPI
catalog — so Cursor *could* clamp, not merely passthrough. Combined with the
loud invalid-id rejection, an effort word encoded into the id is safe either way.

## 6. Gemini — from the spike

Reasoning effort is a **numeric** `thinkingConfig.thinkingBudget` in
`settings.json`, orthogonal to argv and not a level word. Mapping the Ralphy
level ladder onto budget numbers is a design choice the ADR defers.
📖 doc-only, never exercised.

## 7. Kimi — confirmed

No `model_reasoning_effort` analog; the adapter discards the parameter
(`let _ = effort;`) and passes `""` to `emit::planning`.

---

## 8. Residuals

1. ~~**Cursor `[effort=high]` bracket**~~ — **RESOLVED** (§5): probed live via WSL.
   Bracket is help-documented, suffix confirmed via `--list-models`, and Cursor
   turns out to publish a **free per-model effort catalog**.
3. ~~**Claude per-model clamp**~~ — **RESOLVED, bounded** (§2): Claude accepts a
   valid level on any model with no per-model signal and publishes no catalog, so
   a per-model clamp is not buildable. Direct map + value-enum validation is the
   ceiling; a silent per-model degrade, if it exists, is unobservable and
   uncorrectable.
2. **Gemini `thinkingBudget` mapping** — still open, but it is a **design
   decision, not a probe**: mapping five level rungs onto numeric budgets has no
   empirical answer to find. Gemini stays *No-op* until the mapping is designed.
