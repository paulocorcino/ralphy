// Demo seed: the project list — fabricated repos with branches, dirty flags and
// nested file trees, the shape a backend delivers as JSON.
//
// Lives OUTSIDE `assets/ui` on purpose: `lib.rs` embeds that directory whole via
// `include_dir!`. Loaded only by the `file://` demo, so a daemon neither ships
// nor serves any of it (#300, ADR-0036).
//
// `loadRepos()` overwrites the shell's list with the real registry in daemon
// mode; this survives only as the `file://` standalone fallback, where there is
// no daemon to fetch from.
window.WB_SEED_PROJECTS = [
  {
    slug: "lingopilot",
    branch: "main",
    // local branches the picker offers (impl: `git branch`, current marked)
    branches: [
      "main",
      "feat/xterm-v6-webgl",
      "feat/chat-streaming",
      "feat/onboarding-flow",
      "fix/auth-redirect",
      "fix/db-pool-leak",
      "chore/deps-bump",
      "chore/ci-cache",
      "experiment/rag-eval",
    ],
    dirty: true, // uncommitted changes → the modal warns before checkout
    state: "live",
    remote: "github",
    tree: [
      {
        title: "src",
        folder: true,
        expanded: true,
        children: [
          { title: "app", folder: true, children: [{ title: "page.tsx" }, { title: "layout.tsx" }] },
          { title: "components", folder: true, children: [{ title: "Chat.tsx" }, { title: "Sidebar.tsx" }] },
          { title: "lib", folder: true, children: [{ title: "db.ts" }, { title: "auth.ts" }] },
        ],
      },
      { title: "prisma", folder: true, children: [{ title: "schema.prisma" }] },
      { title: "package.json" },
      { title: "next.config.ts" },
      { title: "tsconfig.json" },
      { title: "logo.png" },
      { title: "README.md" },
    ],
  },
  {
    slug: "fincal",
    branch: "feat/triage",
    branches: ["main", "feat/triage", "feat/reconcile", "fix/csv-import"],
    dirty: false,
    state: "idle",
    remote: "github",
    tree: [
      { title: ".ralphy", folder: true, children: [{ title: "plan.md" }, { title: "triage-draft.json" }] },
      {
        title: "docs",
        folder: true,
        children: [
          { title: "adr", folder: true, children: [{ title: "0001-vocabulary.md" }] },
          { title: "issues", folder: true, children: [] },
        ],
      },
      { title: "src", folder: true, children: [{ title: "index.ts" }, { title: "styles.css" }] },
      { title: "CONTEXT.md" },
      { title: "package.json" },
    ],
  },
  {
    slug: "ralphy",
    branch: "feat/xterm-v6-webgl",
    branches: ["main", "feat/xterm-v6-webgl", "feat/daemon-mode", "feat/assignee-filter"],
    dirty: false,
    state: "idle",
    remote: "github",
    tree: [
      {
        title: "crates",
        folder: true,
        children: [
          { title: "ralphy-cli", folder: true, children: [{ title: "main.rs" }] },
          { title: "ralphy-core", folder: true, children: [{ title: "lib.rs" }] },
          { title: "ralphy-daemon", folder: true, children: [{ title: "protocol.rs" }, { title: "dispatch.rs" }] },
        ],
      },
      { title: "docs", folder: true, children: [{ title: "adr", folder: true, children: [{ title: "0035-daemon-ui-visual-language.md" }] }] },
      { title: "Cargo.toml" },
    ],
  },
  {
    slug: "bioledger",
    branch: "main",
    branches: ["main", "wip/ocr-tuning"],
    dirty: false,
    state: "offline",
    remote: "local", // never pushed anywhere — lives only on this disk
    tree: [
      { title: "src", folder: true, children: [{ title: "ocr.ts" }] },
      { title: "tests", folder: true, children: [{ title: "ocr.test.mjs" }] },
      { title: "notes.md" },
      { title: "package.json" },
    ],
  },
];
