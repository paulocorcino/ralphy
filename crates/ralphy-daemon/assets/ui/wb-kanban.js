/* ---------------------------------------------------------------------------
   ralphy workbench shell — Kanban model

   The Kanban is the backlog as a board: every GitHub issue of the open project,
   placed in one of four columns by the SAME judgment ralphy uses. It is a
   read-only lens on the tracker — the daemon never edits an issue's prose. The
   ONE mutation the board allows is changing labels (which is how an issue moves
   between columns); everything else routes to GitHub via an "Open on GitHub"
   link for real editing.

   Columns (an issue lands in exactly one, precedence top-down):
     • Closed          — state = closed, grouped by close reason (completed /
                         not planned), mirroring GitHub's `stateReason`.
     • Ready for human — carries `ready-for-human` (HITL): parked on a person
                         (ADR-0014/0016). Human gate outranks agent-eligibility,
                         exactly as the runner's queue precedence does.
     • Ready for agent — carries `ready-for-agent` OR `AFK` (same intent: fully
                         specified, an AFK agent can pick it up).
     • Backlog         — everything else still open (needs-triage, needs-split,
                         bug/enhancement, or no label at all).

   Ordering:
     • The two Ready columns are ordered by the dependency graph — a JS port of
       `sort_queue_in_graph` (crates/ralphy-core/src/blocked.rs): Kahn's
       algorithm over `## Blocked by` edges, ascending issue number as the
       tie-break, blockers walked transparently through open out-of-queue nodes,
       closed blockers pruned (satisfied), a retired bundle's `## Parent`
       children standing in for it. The order shown IS the order the runner would
       execute — the board and the queue can never disagree.
     • Backlog stays in issue order (newest-first by default) with a search box,
       a label filter, and a sort control — a flat, compact list.

   Running signal: an issue that is the *active* node of a live run (see
   wb-runs.js / WB_RUNS) carries a run pill on its card — the agent's face + the
   live status glyph + the phase — so "what's executing right now" reads at a
   glance, in whichever column the issue sits.

   This file holds:
     • WB_KANBAN  — the seed (a backend replaces it live from the tracker),
     • WBKanban   — pure helpers (column classification, graph order, running
                    cross-ref, label metadata, filter/sort).
   Faithful sources: labels + colors = the repo's `gh label list`; close reasons
   = GitHub `stateReason`; graph order = ralphy-core/src/blocked.rs; run glyphs =
   window.WBRun (wb-runs.js).
--------------------------------------------------------------------------- */

window.WBKanban = {
  // GitHub label vocabulary → { color, short }. This is only the FALLBACK seed:
  // `boardLabels[slug]` (the repo's live `gh label list`) wins when the daemon
  // answers, so the seed matters for the static demo and for a label the API
  // returns without a colour. Ralphy's own labels are kept in step with
  // `ralphy_label_specs` (ralphy-core/src/github/labels.rs) and grouped by its
  // families — green = go/queue, purple = blocked on a person, amber = triage,
  // red = the run stopped here. `short` is a compact chip label where the full
  // name is long. Unknown labels fall back to a neutral chip.
  LABELS: {
    "ready-for-agent": { color: "#0E8A16", short: "ready · agent" },
    AFK: { color: "#9BE9A8", short: "AFK" },
    "ready-for-human": { color: "#5319E7", short: "ready · human" },
    HITL: { color: "#8957E5", short: "HITL" },
    "needs-info": { color: "#D4C5F9", short: "needs-info" },
    "needs-triage": { color: "#FBCA04", short: "needs-triage" },
    "triage-agent": { color: "#FFE0A6", short: "triage-agent" },
    "stop-before": { color: "#D93F0B", short: "stop-before" },
    "needs-split": { color: "#E99695", short: "needs-split" },
    "needs-human-review": { color: "#BFD4F2", short: "needs review" },
    bug: { color: "#D73A4A", short: "bug" },
    enhancement: { color: "#A2EEEF", short: "enhancement" },
    documentation: { color: "#0075CA", short: "docs" },
    question: { color: "#D876E3", short: "question" },
    duplicate: { color: "#CFD3D7", short: "duplicate" },
    wontfix: { color: "#E6E6E6", short: "wontfix" },
    invalid: { color: "#E4E669", short: "invalid" },
    "good first issue": { color: "#7057FF", short: "good first issue" },
    "help wanted": { color: "#008672", short: "help wanted" },
  },
  // The four columns, in render order (left → right), with their heading + icon.
  COLUMNS: [
    { id: "backlog", title: "Backlog", lucide: "inbox" },
    { id: "agent", title: "Ready for agent", lucide: "bot" },
    { id: "human", title: "Ready for human", lucide: "user-round" },
    { id: "closed", title: "Closed", lucide: "check-check" },
  ],

  labelMeta(name) {
    return this.LABELS[name] || { color: "#6b5f52", short: name };
  },
  labelColor(name) {
    return this.labelMeta(name).color;
  },
  // A chip's text color: dark ink on a light chip, light ink on a dark one, so a
  // wontfix-white and a triage-maroon both stay legible.
  labelInk(name) {
    const hex = this.labelColor(name).replace("#", "");
    const r = parseInt(hex.slice(0, 2), 16),
      g = parseInt(hex.slice(2, 4), 16),
      b = parseInt(hex.slice(4, 6), 16);
    // perceived luminance
    return 0.299 * r + 0.587 * g + 0.114 * b > 150 ? "#2b2620" : "#e9e1d4";
  },

  // Which column an issue belongs to — the runner's precedence, as a lens:
  // closed first, then the human gate, then agent-eligibility, else backlog.
  columnOf(iss) {
    if (iss.state === "closed") return "closed";
    const L = iss.labels || [];
    if (L.includes("ready-for-human")) return "human";
    if (L.includes("ready-for-agent") || L.includes("AFK")) return "agent";
    return "backlog";
  },

  // GitHub's close reason as a short badge label.
  closeLabel(iss) {
    if (iss.state !== "closed") return "";
    return iss.reason === "not_planned" ? "not planned" : "completed";
  },

  // The one open blocker check the board needs: is `#n` still open in this
  // project? (Mirrors the runtime gate — a closed blocker is satisfied.)
  openBlockers(iss, all) {
    const openSet = new Set(all.filter((i) => i.state === "open").map((i) => i.number));
    return (iss.blockedBy || []).filter((n) => openSet.has(n));
  },

  // --- graph order (port of sort_queue_in_graph / Kahn) -----------------
  // Order `queue` so every issue comes after the queue members it depends on,
  // ascending number as the tie-break. `all` is the project's full issue set
  // (for walking edges through open out-of-queue blockers, pruning closed ones,
  // and substituting a retired bundle's `## Parent` children).
  orderGraph(queue, all) {
    const inQueue = new Set(queue.map((i) => i.number));
    const openNums = new Set(all.filter((i) => i.state === "open").map((i) => i.number));
    const blockedOf = new Map();
    for (const i of all) blockedOf.set(i.number, i.blockedBy || []);
    for (const i of queue) if (!blockedOf.has(i.number)) blockedOf.set(i.number, i.blockedBy || []);
    // childrenOf[parent] = queue members declaring `## Parent` #parent (stand-ins
    // for a retired/closed bundle).
    const childrenOf = new Map();
    for (const i of queue) {
      if (i.parent == null) continue;
      if (!childrenOf.has(i.parent)) childrenOf.set(i.parent, []);
      childrenOf.get(i.parent).push(i.number);
    }
    // deps[x] = queue members that must precede x.
    const deps = new Map();
    for (const i of queue) {
      const acc = new Set();
      const seen = new Set();
      const stack = [i.number];
      while (stack.length) {
        const node = stack.pop();
        if (seen.has(node)) continue; // already expanded — also breaks cycles
        seen.add(node);
        for (const n of blockedOf.get(node) || []) {
          if (n === i.number) continue;
          if (inQueue.has(n)) acc.add(n); // terminal in-queue predecessor
          else if (openNums.has(n)) stack.push(n); // transparent: keep walking
          else {
            const ch = childrenOf.get(n); // closed bundle → its children stand in
            if (ch) for (const c of ch) if (c !== i.number) acc.add(c);
          }
        }
      }
      deps.set(i.number, acc);
    }
    // Kahn, smallest ready number first (ascending tie-break); a cycle's
    // remainder is appended ascending (the runtime gate owns correctness).
    const byNum = new Map(queue.map((i) => [i.number, i]));
    const placed = new Set();
    const out = [];
    while (byNum.size) {
      const keys = [...byNum.keys()].sort((a, b) => a - b);
      const ready = keys.find((n) => [...(deps.get(n) || [])].every((d) => placed.has(d)));
      if (ready == null) {
        for (const n of keys) out.push(byNum.get(n));
        break;
      }
      placed.add(ready);
      out.push(byNum.get(ready));
      byNum.delete(ready);
    }
    return out;
  },

  // --- running cross-ref (against the CALLER's runs, via window.WBRun) ----
  // `projectRuns` is whatever the panel holds: live snapshots in daemon mode,
  // the WB_RUNS seed under `file://` (#300). Nothing is read from the seed here.
  // If `number` is the *active* node of one of the project's live runs, return a
  // descriptor for the card's run pill; else null. Only the actively-worked
  // issue (planning / executing / sleeping) is flagged — a run's pending or
  // already-terminal members don't clutter the board.
  runningFor(number, projectRuns) {
    for (const r of projectRuns || []) {
      const iss = (r.issues || []).find((x) => x.number === number);
      if (!iss) continue;
      const st = window.WBRun.issueState(r, iss);
      if (st === "planning" || st === "executing" || st === "sleep") {
        return { runid: r.runid, face: r.face, agent: r.agent, state: st, glyph: window.WBRun.GLYPH[st], phase: r.phase };
      }
    }
    return null;
  },

  // --- refresh policy (#301) ---------------------------------------------
  // The board fold is NOT an in-daemon read: each load spawns a CLI that makes
  // several tracker calls, so the two machine-driven triggers must coalesce —
  // `runs` arrives on every snapshot write (~every few hundred ms during a run)
  // and `visible` fires on every alt-tab. The two operator-driven ones (`manual`,
  // `label`) skip the gap entirely: the operator asked, and lag would read as a
  // broken control.
  REFRESH_MIN_GAP_MS: 5000,
  REFRESH_BACKSTOP_MS: 120000,

  // Should a refresh happen? A pure function of the trigger, the time since the
  // last load, and the board/document state — no board state is read here, which
  // is what makes the policy testable in isolation. The trigger vocabulary is
  // closed: an unrecognised one never refreshes. `focused` is consulted ONLY by
  // `backstop` — a slow tick on a background tab is exactly the blind polling
  // this design refuses.
  shouldRefresh({ trigger, sinceMs, boardOpen, docVisible, focused = true }) {
    if (!boardOpen || !docVisible) return false;
    switch (trigger) {
      case "manual":
      case "label":
        return true;
      case "visible":
      case "runs":
        return sinceMs >= this.REFRESH_MIN_GAP_MS;
      case "backstop":
        return !!focused && sinceMs >= this.REFRESH_BACKSTOP_MS;
      default:
        return false;
    }
  },

  // --- filter / sort (Backlog) ------------------------------------------
  matches(iss, q) {
    if (!q) return true;
    const s = q.trim().toLowerCase();
    if (!s) return true;
    return (
      String(iss.number).includes(s) ||
      (iss.title || "").toLowerCase().includes(s) ||
      (iss.body || "").toLowerCase().includes(s) ||
      (iss.labels || []).some((l) => l.toLowerCase().includes(s))
    );
  },
  hasLabelFilter(iss, label) {
    if (label === "__all") return true;
    if (label === "__none") return (iss.labels || []).length === 0;
    return (iss.labels || []).includes(label);
  },
  SORTS: [
    { id: "num-desc", label: "newest" },
    { id: "num-asc", label: "oldest" },
    { id: "updated", label: "recently updated" },
    { id: "title", label: "title A–Z" },
  ],
  sortBacklog(list, sort) {
    const a = list.slice();
    switch (sort) {
      case "num-asc":
        return a.sort((x, y) => x.number - y.number);
      case "updated":
        return a.sort((x, y) => (y.updated || "").localeCompare(x.updated || ""));
      case "title":
        return a.sort((x, y) => (x.title || "").localeCompare(y.title || ""));
      case "num-desc":
      default:
        return a.sort((x, y) => y.number - x.number);
    }
  },

  fmtDate(iso) {
    if (!iso) return "";
    const d = new Date(iso);
    if (isNaN(d)) return iso;
    return d.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" });
  },
};
