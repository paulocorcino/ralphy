// The console menu's rows, folded from the daemon's adapter roster
// (GET /api/agents), the live session list and the open repo. Pure: no DOM, no
// fetch, no mutation of its inputs — unit-tested in ui-tests/wb-agents.test.mjs.
(function (window) {
  "use strict";

  // Verbatim in the menu's title attribute AND pinned by both test layers.
  const NEEDS_REPO = "select a repo first — an agent needs one to work in";
  const NOT_INSTALLED = "not installed here";

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

  function rowFor(kind, label, plain, digit, available, reason, sessions, openSlug) {
    // A count drawn from another repo would offer to reach a session with a
    // different working directory — a row that lies about what its click does.
    const scope = plain ? openSlug || "~" : openSlug;
    const mine = (sessions || []).filter(
      (s) => s && s.agent === kind && s.repo === scope,
    );
    const needsRepo = !plain && !openSlug;
    const unavailable = !plain && available === false;
    const disabled = needsRepo || unavailable;
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
      needsRepo,
      unavailable,
      title: needsRepo ? NEEDS_REPO : unavailable ? reason || NOT_INSTALLED : "",
      tryAnyway: unavailable && !needsRepo,
      live: mine.length,
      // The plain console ALWAYS launches: a free shell is idempotent and the
      // operator opens one per task, so reaching an existing one would be
      // surprising. It must therefore never report `attach` — the row's action
      // is what the menu renders its "reach" affordances from, and a row that
      // said "attach" while the click launched would lie about its own click.
      action: !plain && mine.length ? "attach" : "launch",
      sessionId: plain ? null : sessionId,
    };
  }

  function menuRows({ roster, sessions, openSlug } = {}) {
    const rows = (roster || []).map((r) =>
      rowFor(
        r.id,
        r.label || r.id,
        false,
        r.accelerator,
        r.available,
        r.reason,
        sessions,
        openSlug,
      ),
    );
    // The plain shell is not a vendor adapter, so it never enters the daemon's
    // roster; the menu appends it last on digit 0.
    rows.push(rowFor("console", "console", true, "0", true, null, sessions, openSlug));
    return rows;
  }

  function canLaunch(row, tryAnyway = false) {
    return !!row && !row.needsRepo && (!row.unavailable || tryAnyway);
  }

  function consoleIntent(row, { fresh = false, tryAnyway = false } = {}) {
    if (!canLaunch(row, tryAnyway)) return null;
    return !row.plain && row.action === "attach" && !fresh ? "attach" : "launch";
  }

  function rosterUrl(repo) {
    return repo ? `/api/agents?repo=${encodeURIComponent(repo)}` : "/api/agents";
  }

  function runRows(roster) {
    return (roster || []).map((row) => ({
      id: row.id,
      label: row.label || row.id,
      available: row.available !== false,
      title: row.available === false ? row.reason || NOT_INSTALLED : "",
    }));
  }

  function rosterState(roster, repo) {
    const rows = Array.isArray(roster) ? roster.slice() : [];
    return { repo: repo || null, roster: rows, agents: runRows(rows) };
  }

  window.WBAgents = {
    menuRows,
    canLaunch,
    consoleIntent,
    rosterUrl,
    rosterState,
    runRows,
    DEMO_ROSTER,
    NEEDS_REPO,
    NOT_INSTALLED,
  };
})(window);
