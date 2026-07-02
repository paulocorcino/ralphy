# Plan prompt sources (canonical template + variant overlays)

The four plan prompt artifacts the adapters embed via `include_str!` —
`../prompt.plan.md` (claude), `../prompt.plan.codex.md` (codex),
`../prompt.plan.opencode.md` (opencode), and `../prompt.plan.staged.md`
(staged planning, claude-only today) — are **generated** from the files in
this directory. Never edit those artifacts directly.

- `template.md` — the canonical shared body. Eight `{{slot}}` placeholder lines
  mark where the variant-specific blocks go:
  - `{{execution-model}}` — the `## Execution model` tier block (empty for
    opencode, which has no tier);
  - `{{self-review-step}}` — the self-review checklist step, phrased in each
    vendor's own invocation idiom (subagent / Codex dispatch / inline skill);
  - `{{self-review-guidance}}` — the Rules bullet(s) describing the penultimate
    self-review step and the final green-gate step;
  - `{{ledger-example}}` — the canonical `[verified]` ledger example line
    (claude's names `cargo test`/`parse_ledger`; the others are vendor-neutral);
  - `{{planning-mode-intro}}` — extra preamble paragraph for a planning mode
    (staged names the `stagedplan` label and the `staged-plan` skill; empty for
    the three vendor prompts);
  - `{{skill-invocation}}` — the task-step note that invokes the `staged-plan`
    skill non-interactively (`STAGED_PLAN_NONINTERACTIVE=1`); empty for the
    three vendor prompts;
  - `{{stages-section}}` — the `## Stages` skeleton section (staged only);
  - `{{mode-rules}}` — trailing Rules bullets specific to a planning mode
    (staged: authoritative-artifact, stage ordering, and the bundle-rule
    override); empty for the three vendor prompts.
- `overlay.<variant>.md` — one file per variant (claude, codex, opencode,
  staged), each slot's verbatim content under a `<!-- slot: name -->` marker
  line. An empty slot (two adjacent markers) deliberately omits that block.

Staged planning is claude-only today (`ralphy-agent-claude` selects
`prompt.plan.staged.md` on the `stagedplan` label); its reviewer idiom lives in
`overlay.staged.md`'s `self-review-step` slot. A future non-claude staged run
must add its own overlay + artifact tuple in `prompt_assembly.rs` — it cannot
silently inherit the claude idiom.

## Editing a prompt

1. Shared prose → edit `template.md`. Variant block → edit the overlay(s).
2. Regenerate the artifacts:

   ```sh
   RALPHY_REGEN_PROMPTS=1 cargo test -p ralphy-core --test prompt_assembly
   ```

3. Commit the sources AND the regenerated artifacts together.

The `prompt_assembly` test in `ralphy-core` fails whenever an artifact no
longer byte-matches template + overlay, so a shared rule fixed in one artifact
(instead of in the template) cannot silently diverge from the others.
