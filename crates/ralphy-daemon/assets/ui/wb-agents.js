// The console menu's rows, folded from the daemon's adapter roster
// (GET /api/agents), the live session list and the open repo. Pure: no DOM, no
// fetch, no mutation of its inputs — unit-tested in ui-tests/wb-agents.test.mjs.
(function (window) {
  "use strict";

  // Verbatim in the menu's title attribute AND pinned by both test layers.
  const NEEDS_REPO = "select a repo first — an agent needs one to work in";

  // Demo-only seed for the static file:// walkthrough, where no daemon answers
  // /api/agents. Deliberately NOT pinned by any test: pinning it would force a
  // frontend edit on every vendor onboarding, the exact cost this module removes.
  const DEMO_ROSTER = [
    { id: "claude", label: "claude", accelerator: "1" },
    { id: "codex", label: "codex", accelerator: "2" },
    { id: "opencode", label: "opencode", accelerator: "3" },
    { id: "kimi", label: "kimi", accelerator: "4" },
    { id: "copilot", label: "copilot", accelerator: "5" },
    { id: "cursor", label: "cursor", accelerator: "6" },
    { id: "gemini", label: "gemini", accelerator: "7" },
  ];

  function rowFor(kind, label, plain, digit, sessions, openSlug) {
    // A count drawn from another repo would offer to reach a session with a
    // different working directory — a row that lies about what its click does.
    const scope = plain ? openSlug || "~" : openSlug;
    const mine = (sessions || []).filter(
      (s) => s && s.agent === kind && s.repo === scope,
    );
    const disabled = !plain && !openSlug;
    // Lowest id, so the row's action cannot change under the operator's cursor
    // as new sessions appear.
    const sessionId = mine.length
      ? mine.reduce((lo, s) => (s.id < lo ? s.id : lo), mine[0].id)
      : null;
    return {
      kind,
      label,
      plain,
      digit,
      disabled,
      title: disabled ? NEEDS_REPO : "",
      live: mine.length,
      action: mine.length ? "attach" : "launch",
      sessionId,
    };
  }

  function menuRows({ roster, sessions, openSlug } = {}) {
    const rows = (roster || []).map((r) =>
      rowFor(r.id, r.label || r.id, false, r.accelerator, sessions, openSlug),
    );
    // The plain shell is not a vendor adapter, so it never enters the daemon's
    // roster; the menu appends it last on digit 0.
    rows.push(rowFor("console", "console", true, "0", sessions, openSlug));
    return rows;
  }

  window.WBAgents = { menuRows, DEMO_ROSTER, NEEDS_REPO };
})(window);
