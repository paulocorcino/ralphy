# Robust read-time pricing: one canonical source, a unified token domain, priced by (provider, model)

Ralphy's read-time USD projection graduates from a hand-maintained hardcoded
table to a **models.dev-backed, locally-cached price table**, keyed on
`(provider, model)`, over a **unified `Tokens` domain** that both the run ledger
(ADR-0008) and the interactive **usage scan** (ADR-0033) converge to. This
**amends ADR-0008** — it reverses D8's rejection of network price-sync (while
upholding D2's "tokens are the truth, USD is a read-time estimate, never
stored") and adds a `provider` field to the D6 record — and **extends ADR-0033**
(the interactive record gains `provider`, and the two paths share one counting
normalization). It deliberately does **not** adopt tokscale's token-*counting*
model, its multi-source fuzzy resolver, or its universal session scanner.

Status: proposed.

## Why now

Under ADR-0008 the price table was "indicative, not asserted by tests, captured
2026-06" on purpose: for a single operator on a subscription, USD is a *relative*
efficiency proxy, so the precision bar was deliberately low (D8). Two things
raised the bar. First, the **daemon / control-plane** work (ADR-0032/0033) makes
usage a **fleet-wide, cross-vendor, consolidated** figure surfaced in a web
summary — comparing `opus` vs `gpt-5.5` vs `k2p6` spend across projects, where a
stale hardcoded rate silently misleads. Second, **OpenCode's open-ended model
space** (any provider/model the operator points it at) cannot be covered by a
hardcoded table at all — its custom IDs are exactly the "unknown model → $0
lie" D8 guards against. The counterfactual "what would this have cost on metered
API" is the investment/efficiency measure we now want to compute *accurately*.

## D1 — Scope is price accuracy + domain unification; token *counting* stays as-is

The work is entirely on the **price** side and the **domain** it projects over.
Ralphy's per-adapter token *counting* (ADR-0008 D5) is **not** changed — a
confrontation against tokscale found Ralphy's counting is, for the vendors it
drives, at least as correct: tokscale keeps `reasoning_output_tokens` as a
distinct additive bucket, but Codex reports `total_tokens = input + output` with
reasoning already **inside** `output`, so adding it double-counts — the exact
trap ADR-0008 D5 avoids. Porting tokscale's counting would regress Codex.
The one optional counting hardening worth taking (orthogonal to pricing): read
Codex's `cache_read_input_tokens` as an alias for `cached_input_tokens` and clamp
`cache_read ≤ input`, guarding a field rename or malformed snapshot.

## D2 — One `Tokens` value object; `Usage` becomes `Tokens` + attribution

Today two normalized shapes exist for the same concept: `ralphy_core::Usage
{input, output, cache_read, cache_creation, model}` (the ledger path) and
`ralphy_usage_scan::Tokens {input, output, cache_read, cache_creation}` + an
enclosing `InteractiveRecord` (the scan path). They are field-aligned by
convention but are two types. This ADR **unifies them on a pure `Tokens` counter**
(the four counts, nothing else); `model` and `provider` ride as **attributes of
the enclosing record**, never inside the counter. `Usage` becomes `Tokens` +
`model` + `provider`. Public API stays stable via re-export (CLAUDE.md rule). The
payoff: pricing projects over one type, and the web summary sums run and
interactive spend without reconciling two shapes.

## D3 — Priced usage is still a read-time projection; the key is (provider, model)

USD is computed **when a report or the web summary is read**, never written —
ADR-0008 D2/D8 are upheld unchanged; only the *source* of the price table
changes. The projection is `price(tokens, provider, model) -> Option<Usd>`.
The key gains **provider** because OpenCode runs the same model under different
providers at different prices (`k2p6` via Moonshot vs a Fireworks router), and
its records carry `providerID`. Provider is **explicit** for OpenCode, **synthesized**
for single-provider vendors (`anthropic` for Claude, `openai` for Codex) so the
key is uniform on every path. The unknown-model rule survives and sharpens: an
absent price is **`None`, never `$0`**, and `None` is reserved for an unknown
**model** — an absent or unmapped **provider** degrades to a model-part lookup
first, never straight to `None`.

## D4 — Two-step deterministic resolution: a curated alias map, then models.dev

Resolution is **two-step and never fuzzy**. Live data from the operator's own
`opencode.db` (below) proved the naive "OpenCode ids resolve directly against
models.dev" premise **false**: OpenCode's `providerID` is a *subscription-plan
slug* (`kimi-for-coding`, `zai-coding-plan`) and its `modelID` a *short alias*
(`k2p6`, `glm-5.2`, `big-pickle`) — the pair exists in no upstream catalog. So:

1. **Alias → canonical.** A **curated, operator-extensible alias map** rewrites
   `(providerID, modelID)` to a canonical `(provider, model)` — e.g.
   `(kimi-for-coding, k2p6)` → `(moonshotai, kimi-k2.6)`. This is curated
   knowledge, not derivable from the slug; it ships with entries for the common
   subscription plans and the operator extends it in `pricing.toml`. This is the
   **primary path for OpenCode**.
2. **Canonical → price.** The canonical `(provider, model)` is priced against
   **models.dev** (the metered majors — Claude `anthropic/…`, Codex `openai/…`,
   and any mainstream metered model — resolve here directly, skipping step 1),
   or the operator prices the alias directly in `pricing.toml`.

models.dev is thus the network source for **freshness of the metered majors**,
**not** the resolver of OpenCode's plan aliases — for a subscription-plan OpenCode
setup, the curated + operator layer does all the real work. This still reverses
ADR-0008 D8's network-sync rejection (the majors now track upstream), with the
reason recorded (Why-now). Ralphy does **not** port tokscale's ~5500-line fuzzy
resolver: it exists to compensate for tokscale receiving messy ids from ~60 CLIs
*without provider context*, and even that would not resolve `k2p6` — only the
curated alias does. Only **deterministic** normalization is kept (release-date
strip, separator). LiteLLM and OpenRouter are **not** pulled in.

- *Validated (live against the operator's real `~/.local/share/opencode/opencode.db`,
  2026-07-11)*: 367 assistant messages, **every one** carrying `providerID` **and**
  `modelID` at the message level (so capture is trivial, D3). Values were
  `providerID` ∈ {`kimi-for-coding`, `zai-coding-plan`, `opencode`} and `modelID`
  ∈ {`k2p6`, `glm-5.2`, `k2p5`, `big-pickle`} — 100% operator aliases, 0% direct
  models.dev keys. This is the empirical basis for the two-step resolution and for
  the first-class operator override (D6).

## D5 — Long-context tiers: the structure supports them, a curated layer fills them

Anthropic's flagship models charge ~2× above a 200K-token context; models.dev's
cost shape is **flat** (no tier fields — those live in LiteLLM). Rather than pull
LiteLLM's whole dataset back in for a handful of models, the `PriceTable`
**structure** carries optional above-threshold fields, but only the **curated /
hardcoded fallback layer** populates them, for the 2–3 models where the ~2× is
material. Precedence: operator `pricing.toml` (may carry tiers) > models.dev
(flat base) > curated/hardcoded (tiers where they matter). If tiers ever become
material across many models, LiteLLM is reconsidered — but *with* provider context,
not with the fuzzy engine.

## D6 — A shared `ralphy-pricing` crate; the daemon is the only fetcher

The read-time engine moves out of `ralphy-cli` into a shared crate,
`ralphy-pricing`, so the CLI footer, `ralphy usage`, and (via the daemon) the web
summary all price through **one** implementation. Only the **daemon** — resident,
off the run's hot path — fetches models.dev over the network (honoring ADR-0008
D1's no-network-in-the-hot-path stance); the run and CLI **never** touch the
network. They read **cache-or-fallback**, precedence: operator `pricing.toml` >
fresh cache > stale cache > embedded hardcoded defaults. The hardcoded table thus
never dies — it is the always-present offline floor, so a missing network yields
an approximate number, never `~$?` (only an unknown *model* does). A machine whose
daemon has run once keeps a cache that even standalone `ralphy run` reads; a
machine where the daemon never ran stays on the hardcoded floor — accepted.

Because D4's live data makes the operator override the **primary** OpenCode path
(not a rare fallback), `pricing.toml` graduates from its current bare-model,
per-1M, flat shape to a **first-class override**: keyed on `(provider, model)`
with a bare-model fallback, an entry either **aliases** to a canonical
`(provider, model)` (then priced by models.dev) or **prices directly**, accepting
rates as **per-1M or per-token** and optional long-context tiers — the ergonomics
worth borrowing surgically from tokscale's custom-pricing format. The
"unknown model — add to `pricing.toml`" hint (ADR-0008 D8) becomes a prominent,
actionable surface, not a buried warning, since for a custom OpenCode setup it is
the expected first-run experience.

## D7 — A single local cache file, atomically written, refreshed on a 24h TTL

The cache is `~/.ralphy/pricing-cache/models-dev.json` (home resolved via
`USERPROFILE`/`HOME`, matching the ledger and `pricing.toml`), a timestamp-wrapped
JSON snapshot written **atomically** (temp file + rename — never delete the
canonical before writing). TTL is **24h** (model prices change rarely; models.dev
is stable), governing when the daemon refreshes, not a hard expiry — a stale cache
still outranks the hardcoded floor because it is real upstream data. The daemon
refreshes on start and every TTL.

## D8 — Two roles, two canonical sources, one calculation

The measurement splits by responsibility, so the two paths never consolidate the
same value twice:

- The **run** process stays operator-facing (ADR-0008 D11): it reports its own
  run and reads the ledger for the operator's project total. It never consolidates
  the fleet.
- The **daemon** owns the **analytic consolidation** for the web: it reads the
  **ledger** (the canonical source of *run* spend) **and** runs the **scan** (the
  canonical source of *interactive* spend), and prices both through
  `ralphy-pricing`.

The dedup boundary is **`session_id`**: the scan already excludes
`run_session_ids`, so a session is in exactly one source — consolidated spend is
`ledger(runs) ⊎ scan(interactive − runs)`, no overlap by construction. What must
be **identical** across the two paths is the *calculation*: the per-vendor
raw→`Tokens` normalization becomes **one canonical shared function per vendor**
that both the adapter and the scan call (eliminating today's two implementations,
which diverge — e.g. Claude dedup is `message.id` first-wins in the adapter vs
`id:requestId`+MAX in the scan), with a golden parity test as its safety net.

## D9 — A delivery is an issue; interactive usage is project-level overhead

The cost surface divides project spend by **delivery = one issue** — the unit
Ralphy delivers, already keyed on the ledger (`issue`). A delivery's cost is the
sum of its ledger phase lines across **every run** that touched it, **failed
attempts included** (joined by `issue`), so a costly-to-deliver issue shows its
full cost, not just the winning run's. Attribution is honest by construction:
**run** usage is per-issue and *is* fractionable by delivery; **interactive**
usage carries no issue (a human session was never "delivering issue #N"), so it
rolls up as **project-level overhead**, never rationed across deliveries. The web
summary therefore reads two ways: *per delivery* = that issue's run phases; *per
project* = Σ deliveries (run) + interactive overhead.

## D10 — The fleet-canonical price snapshot is deferred, its seam left open

Each machine caches independently, so across the fleet the same model can price
off snapshots refreshed at slightly different times — noise for a consolidated
view. A **fleet-canonical snapshot** (one central table the web prices against)
would remove it, but the control plane is ADR-0032 Phase 2 and not yet built, so
this is **not** built speculatively (mirroring ADR-0008 D9's "leave the seam,
defer the policy"). The seam is ready: `ralphy-pricing` produces a servable
resolved table, so when the control plane exists it either links the crate (if
Rust) or the daemon serves the resolved table to it (if not) — one resolution, one
source, either way.

## Consequences

- `ralphy-core`'s `Usage` is re-expressed as a shared `Tokens` counter + `model`
  + `provider`; the ADR-0008 D6 ledger record and the ADR-0033 `InteractiveRecord`
  both gain `provider` (additive, append-only-safe).
- A new crate `ralphy-pricing` (the read-time engine + curated alias map +
  models.dev fetch + cache); `ralphy-cli/src/pricing.rs` moves into it. A new
  artifact, `~/.ralphy/pricing-cache/models-dev.json`, and a grown `pricing.toml`
  schema (`(provider, model)` keys, alias-or-price entries, per-1M or per-token,
  optional tiers). Neither touches the target repo.
- Per-vendor raw→`Tokens` normalization is extracted to one shared function per
  vendor, called by both the adapter and the scan; a golden parity test guards it.
- Deliberately **not** built: tokscale's token-counting model (would regress
  Codex), its multi-source fuzzy resolver (the curated alias map replaces it —
  fuzzy would not resolve `k2p6` anyway), its
  universal session scanner (Ralphy owns its four vendors' captures), a
  fleet-canonical price snapshot (deferred to the control plane), and a budget
  gate (still ADR-0008 D9's future work).
- ADR-0008 D8 is amended (network sync now allowed, from models.dev, cached,
  off the hot path); D2 (tokens-truth, USD read-time) and D1 (no network in the
  run) are upheld. ADR-0033's scan gains `provider` and shares the counting
  normalization.

## Amendment (#257, 2026-07-21): the Gemini rows and their mandatory key transform

Five Gemini rows join `PriceTable::defaults` — `gemini-3.1-pro-preview`,
`gemini-3-flash-preview`, `gemini-3.1-flash-lite`, `gemini-2.5-pro`,
`gemini-2.5-flash` — at the indicative ai.google.dev list prices captured in
`docs/research/gemini-cli-adapter-spike.md` §4. `gemini-3.5-flash` is NOT added:
the row Cursor already contributed carries the same figures and the key is now
shared by two vendors.

**`ralphy_agent_gemini::price_key` is the mandatory transform** between a model
id a Gemini run recorded and a `PriceTable` lookup. It is not cosmetic:

- `gemini-3-flash` is the CLI's constant for an engine served by the **3.5**
  backend, while the identically spelled row in this table is Cursor's catalogue
  price for Google's *preview* Flash — **3× apart**. Looking up the raw id prices
  a Gemini run at a third of its cost. `price_key` renames it; the table keeps one
  correct row per vendor.
- `gemini-3-pro-preview` is retired for pinning but still costs out, as its
  successor `gemini-3.1-pro-preview` — a historical run record must price.
- The routing aliases (`auto`, `pro`, `flash`, `flash-lite`, `auto-gemini-3`,
  `auto-gemini-2.5`) fold onto the sentinel **`gemini-routed`**, deliberately
  **unpriced**. `auto` is already a Cursor row (grok-4.5 rates), so passing it
  through would attribute a Gemini run to another vendor's engine.

`gemini-3.1-pro-preview-customtools` gets **no row**: it has no published price
(spike Trap 3), and this table reports unpriced (`~$?`) rather than guessing —
even though it is the model that actually served two probe runs. The two Gemma
ids (`gemma-4-31b-it`, `gemma-4-26b-a4b-it`) are unpriced for the same reason:
they are pinnable and pass through `price_key` verbatim, and no list price was
captured for them. Three families are therefore intentionally `~$?` —
`gemini-routed`, `-customtools`, and the Gemma pair.

The adapter already applies the transform to `Usage::model`. **#263, which parses
the stream's usage envelope, must apply it to every key it writes into
`stats.models`** — an unmapped id there re-opens the 3× misattribution.

Known under-bill: these are flat per-model scalars, and Pro prices differently
above a 200 k prompt (Ralphy's charter alone is ~30 k of it), so a long Pro run
is under-billed. Tiered pricing is out of scope here (PRD #252) and lands with the
`ralphy-pricing` crate this ADR already specifies.

## Amendment (slice A): CLI-first models.dev fetch, a machine-refreshed seed floor, the daemon made optional

The body of this ADR is a whole: the models.dev fetch, the `(provider, model)`
domain key, the curated alias map, the `Tokens` unification, the `ralphy-pricing`
crate, and the daemon-as-only-fetcher, landing together. That whole stays
`proposed`. This amendment carves out and **accepts** a first shippable
increment — **slice A** — that delivers the price-*freshness* half without the
domain surgery, and in doing so **revises D3, D6 and D7** on three points. The
remainder (**slice B** — the `(provider, model)` key, the alias map, the `Tokens`
unification, the `ralphy-pricing` crate, tiers D5) stays as this ADR's proposed
future.

### A1 — Scope: freshness + fallback only; the domain is untouched

Slice A swaps the *source* of the majors' prices and replaces the hand-maintained
floor. It keeps the current **per-`model`** keying end to end: the ledger record
(ADR-0008 D6) gains **no** `provider` field, `ralphy_core::Usage` is **not**
unified into `Tokens`, and `(provider, model)` exists only **transiently at
lookup** (below), never stored. This narrows D1–D3 to "price side only, and even
there without the provider key" for now — the honest consequence is that the
OpenCode-alias accuracy that motivated *Why now* is **deferred to slice B**; the
vendor slugs stay hand-priced (A3).

### A2 — Source stays models.dev; the fetch refreshes only the metered majors

D4's choice of models.dev is upheld, and LiteLLM/OpenRouter re-rejected for slice
A specifically: LiteLLM's inconsistent keying is usable only through the
multi-index + fuzzy resolver tokscale builds to tame it (and even that needs the
curated alias map to resolve `k2p6`), OpenRouter publishes *marketplace* price,
not the vendor **list** price the counterfactual wants, and LiteLLM's one edge —
long-context tiers — is out of slice-A scope (D5). models.dev's clean
`provider/model` keying is what makes the resolver unnecessary.

Because the ids the adapters report are **bare** (`claude-opus-4-8`, `gpt-5.5`)
while models.dev keys `provider/model`, the lookup **synthesizes the provider by a
deterministic prefix rule** (`claude-*`→`anthropic`, `gpt-*`→`openai`,
`gemini-*`→`google`, `kimi-*`→`moonshotai`), builds `provider/model`, and looks it
up in the fetched dataset. A hit prices fresh; a miss falls through to the floor.
This is *not* slice B's alias map — it is a rule, not a table, and it only ever
touches lookup, never the record. The date-collision (`anthropic/claude-opus-4-8`
vs a dated catalog id) is closed by applying the existing `strip_release_date`
normalization to **both** sides at ingest.

**Boundary, load-bearing:** the fetch refreshes **only** the metered majors that
synthesize and match. Every vendor-internal id — `k2p6`, the Cursor families,
`gemini-routed`/`-customtools`, the OpenCode plan slugs — is absent from
models.dev by construction and **stays on the hand floor**. Slice A therefore
ends the manual maintenance of *the majors' numbers*, not of the slug rates.

### A3 — The floor is a machine-refreshed seed + a hand overlay, not hardcoded Rust

D6/D7 assumed the offline floor is the "embedded hardcoded defaults"
(`pricing/defaults.rs`). Slice A **replaces that Rust table** with two
`include_str!` data files so the majors' floor cannot rot by neglect:

- **`assets/pricing/models-dev-seed.json`** — a curated subset of a real
  models.dev snapshot (the providers Ralphy drives), **machine-generated** by a
  checked-in script / scheduled-CI job that opens a diffable PR, **never** by
  `build.rs`. It is consumed as a **pre-seeded stale cache**: identical shape and
  parser to the runtime cache.
- **`assets/pricing/slug-overlay.json`** — the ~dozen vendor-slug/family rates no
  catalog publishes, **hand-maintained**, keyed by the bare id. The script never
  touches it; the human never touches the seed — one owner per file, no merge.

The build stays **hermetic and offline** (`cargo build` only `include_str!`s
checked-in files; no network at compile). Precedence becomes: `pricing.toml` >
fresh cache > stale cache > **seed ⊕ slug-overlay**. `defaults.rs` is retired. The
irreducible residue of hand-work shrinks to the slug rates — which slice B's alias
map finally removes.

### A4 — The run never fetches; `ralphy usage` is the sole trigger; the daemon is optional

This **revises D6's "the daemon is the only fetcher."** ADR-0008 D1's real
invariant — the run never blocks on the network — is upheld strictly: the run
(`plan`/`execute`/`consolidate`) **and the run-end footer** read cache-or-floor
only and **never** touch the network, even post-drain. The **CLI `ralphy usage`**
command is the sole fetch trigger: a **bounded-blocking, best-effort** refresh when
the cache is older than D7's 24h TTL — ~2–3 s timeout, a `--refresh` flag to force,
and on any failure/timeout it serves stale-or-seed and logs once (network never
yields `~$?`; only an unknown *model* does). The footer piggybacks on whatever
fresh cache a prior `usage` wrote. The daemon, **when present**, still refreshes
proactively on start and every TTL — but it is now an **optional accelerator**, not
a requirement, so a machine whose daemon never ran still refreshes on its next
`ralphy usage` rather than being stranded on the floor (the limitation D6 accepted).

### A5 — Fetch is on by default, with an explicit offline opt-out

The refresh is **on by default** — opt-in would leave the seed stale for most
operators and strand the feature. This is defensible because it is a plain
**unauthenticated GET of a public price file**: no operator data, tokens, or usage
leave the machine — it is not telemetry. For air-gapped or egress-restricted
environments, **`RALPHY_PRICING_OFFLINE=1` or `offline = true` in `pricing.toml`**
skips the fetch entirely — it never even *attempts* the connection (which matters
where the attempt itself is a policy event), resolving from `pricing.toml` > cache
> seed. No consent prompt: Ralphy is automation-oriented and a prompt would break
scripting. This honours the opt-in security posture in the correct direction — the
capability ships on, the operator keeps the switch.

### A6 — Implementation boundaries

- **HTTP via `ureq`** (the workspace's sync/blocking client), **not `reqwest`** —
  honouring ADR-0032 §10, which confines tokio/async to the daemon. The
  bounded-blocking `ralphy usage` path is a synchronous one-shot, exactly ureq's
  model. Code stays in `ralphy-cli/src/pricing.rs`; the `ralphy-pricing` crate
  extraction (D6) is **deferred** — slice A has a single consumer (the CLI footer
  and `ralphy usage`); the daemon web summary that justified the crate is slice B.
- **Cache and seed share one shape**: `{ timestamp, data: { "provider/model" →
  {input, output, cache_read, cache_creation} per-1M } }`. Ingest maps models.dev's
  `cache_write` → `cache_creation`, keeps per-1M, and **drops any entry without a
  usable input+output cost** — the `$0`/subscription trap tokscale filters
  (`github_copilot/`), so a `$0` catalog row is never stored as "free". A null
  cache field becomes `0`, **not** a guessed input-rate — that convention lives in
  the hand layer, never the machine catalog.
- **Cache write** = temp file in the same dir → `std::fs::rename` (atomic for
  same-volume files on both OSes); on failure keep the old cache and log once.
- **The ADR-0008 D8 opus oracle splits in two** so the majors moving to the seed
  cannot break the green gate: (a) a frozen inline-fixture **arithmetic** test
  (15/75/1.5/18.75 → 110.25) guarding `cost_usd` math, decoupled from any data;
  (b) a **pipeline** test that `claude-opus-4-8` resolves via synthesis to the
  checked-in seed and prices, its expected value tracking the seed in the same PR.
- **The fetch is tested against a local `127.0.0.1:0` `TcpListener`** serving
  canned responses (tokscale's cross-platform pattern) — no external network in CI.

### A7 — Explicitly out of slice A (still this ADR's proposed future)

The ledger record and `InteractiveRecord` gain no `provider`; `Usage` is not
unified into `Tokens`; the `(provider, model)` key never enters the domain; there
is no curated alias map, no `ralphy-pricing` crate, no long-context tiers (D5), no
`build.rs`, and no daemon change. Each remains specified by the body above and
lands with slice B.

