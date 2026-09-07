// Demo seed: the adapter roster the `file://` walkthrough falls back to when no
// daemon answers /api/agents.
//
// Lives OUTSIDE `assets/ui` on purpose: `lib.rs` embeds that directory whole via
// `include_dir!`. Loaded only by the `file://` demo (#300, ADR-0036).
//
// Deliberately NOT pinned by any test, as ADR-0040 records: pinning it would
// force a frontend edit on every vendor onboarding, the exact cost `wb-agents.js`
// removes. A vendor missing here costs nothing but its absence from the demo.
window.WB_SEED_ROSTER = [
  { id: "claude", label: "claude", accelerator: "1" },
  { id: "codex", label: "codex", accelerator: "2" },
  { id: "opencode", label: "opencode", accelerator: "3" },
  { id: "kimi", label: "kimi", accelerator: "4" },
  { id: "copilot", label: "copilot", accelerator: "5" },
  { id: "cursor", label: "cursor", accelerator: "6" },
  { id: "gemini", label: "gemini", accelerator: "7" },
];
