# Plan prompt sources (canonical template + vendor overlays)

The three plan prompt artifacts the adapters embed via `include_str!` —
`../prompt.plan.md` (claude), `../prompt.plan.codex.md` (codex), and
`../prompt.plan.opencode.md` (opencode) — are **generated** from the files in
this directory. Never edit those artifacts directly.

- `template.md` — the canonical shared body. Four `{{slot}}` placeholder lines
  mark where the vendor-specific blocks go:
  - `{{execution-model}}` — the `## Execution model` tier block (empty for
    opencode, which has no tier);
  - `{{self-review-step}}` — the self-review checklist step, phrased in each
    vendor's own invocation idiom (subagent / Codex dispatch / inline skill);
  - `{{self-review-guidance}}` — the Rules bullet(s) describing the penultimate
    self-review step and the final green-gate step;
  - `{{ledger-example}}` — the canonical `[verified]` ledger example line
    (claude's names `cargo test`/`parse_ledger`; the others are vendor-neutral).
- `overlay.<vendor>.md` — one file per vendor, each slot's verbatim content
  under a `<!-- slot: name -->` marker line. An empty slot (two adjacent
  markers) deliberately omits that block.

## Editing a prompt

1. Shared prose → edit `template.md`. Vendor block → edit the overlay(s).
2. Regenerate the artifacts:

   ```sh
   RALPHY_REGEN_PROMPTS=1 cargo test -p ralphy-core --test prompt_assembly
   ```

3. Commit the sources AND the regenerated artifacts together.

The `prompt_assembly` test in `ralphy-core` fails whenever an artifact no
longer byte-matches template + overlay, so a shared rule fixed in one artifact
(instead of in the template) cannot silently diverge from the other two.
