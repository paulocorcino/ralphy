/* ---------------------------------------------------------------------------
   ralphy workbench shell — the plan.md folds of the Runs panel

   Pure folds over the text of a plan: its steps, its sections, whose plan it is,
   and its verdict. `wb-runs.ts` keeps them as members of `WBRun`.
--------------------------------------------------------------------------- */

/** The two verdicts of `planSummary` the plan pill reads. */
type PlanVerdict = { infeasible: boolean; needsSplit: boolean };

// The status of a plan step from the mark inside its `[ ]`.
function stepStatus(mark: string) {
  if (mark === " ") return "open";
  return mark === "!" ? "noticed" : "checked";
}

// Parse the three checkbox markers out of plan text — the same set the Rust
// side parses (`ralphy_core::plan::count_open_steps` for the open ones). Used by
// `planSummary` over a plan read off disk.
//
// `/\r?\n/`, never `"\n"`: a plan.md written on Windows (or checked out with
// `core.autocrlf`) leaves a trailing `\r` on every line, and `(.*)$` below
// cannot match it — `.` excludes line terminators and `$` only matches the very
// end — so EVERY step silently disappeared and the plan read as "0 steps", i.e.
// infeasible. Measured on this host: a plan the operator could see in the editor
// came back with `steps: 0`.
export function parseSteps(md: string | null | undefined) {
  const out: { text: string; status: string }[] = [];
  (md || "").split(/\r?\n/).forEach((ln) => {
    const [, mark, text] = ln.replace(/^\s+/, "").match(/^- \[([ xX!])\](.*)$/) ?? [];
    if (mark === undefined || text === undefined) return;
    const status = stepStatus(mark);
    const plain = text.replaceAll(/[*_`]/gu, "").trim().replaceAll(/\s+/gu, " ");
    out.push({ text: plain, status });
  });
  return out;
}

// --- plan.md section slicing ------------------------------------------
// Every `## Heading` in the plan, in order (e.g. "Feasible: yes", "Steps"…).
export function headings(md: string | null | undefined) {
  const out: string[] = [];
  (md || "").split("\n").forEach((ln) => {
    const heading = ln.match(/^##\s+(.+?)\s*$/)?.[1];
    if (heading !== undefined) out.push(heading);
  });
  return out;
}

// --- whose plan is this? ------------------------------------------------
// The plan file carries its own issue key: the planner writes the trailer
// `<!-- ralphy-plan: issue=N -->` once every section is complete
// (crates/ralphy-adapter-support/src/resume.rs → `plan_trailer`). The panel
// needs it because the steps block is keyed by `plan.issue` from the snapshot
// (ADR-0047 A1) while the PROSE is a `file.read` of `.ralphy/plan.md` — and
// between issues that file still holds the previous issue's plan.
//
// NOT the Rust rule, deliberately: `plan_is_finalized_for` requires the
// trailer to be the last non-empty line, because it answers "may I resume?".
// This answers "which issue is this prose about?", and the executor appends
// `## Notes & decisions` / `## Handoff` AFTER the trailer while it works — so
// presence anywhere, not position. Do not "fix" this into the resume rule: the
// prose would vanish the moment execution wrote its first note.
export const PLAN_TRAILER_RE = /<!--\s*ralphy-plan:\s*issue=(\d+)\s*-->/g;
export function planTrailerIssue(md: string | null | undefined) {
  if (!md) return null;
  let issue = null;
  // The LAST trailer wins: an appended section can quote an earlier plan, and
  // the planner's own trailer is written at the end of what it wrote.
  for (const m of md.matchAll(PLAN_TRAILER_RE)) issue = Number(m[1]);
  return Number.isFinite(issue) ? issue : null;
}

// Is this plan text the plan FOR `issue`? A plan with no trailer is not: it is
// either mid-write (the planner has not finished) or not a ralphy plan at all,
// and in both cases claiming it belongs to the active issue is the lie.
export function planBelongsTo(md: string | null | undefined, issue: number | null | undefined) {
  if (issue == null) return false;
  return planTrailerIssue(md) === Number(issue);
}

// The body under one `## Heading` (heading line excluded), up to the next `##`.
export function section(md: string | null | undefined, name: string) {
  if (!md || !name) return "";
  const want = name.trim().toLowerCase();
  const out = [];
  let inSec = false;
  // `/\r?\n/` for the same reason as `parseSteps`: a CRLF plan.md would
  // otherwise return a body with a carriage return on every line.
  for (const ln of md.split(/\r?\n/)) {
    const heading = ln.match(/^##\s+(.+?)\s*$/)?.[1];
    if (heading !== undefined) {
      const isTarget = heading.trim().toLowerCase() === want;
      if (inSec && !isTarget) break;
      inSec = isTarget;
      continue; // drop the heading line; the panel chrome shows it
    }
    if (inSec) out.push(ln);
  }
  return out.join("\n").trim();
}

// --- the plan's verdict (the board's plan pill, #350-follow-up) ------------
// A finalized plan is EXECUTED BY THE NEXT RUN (the trailer above is the resume
// signal), so the board has to be able to say one exists, show it, and throw it
// away. This fold is what the pill and the modal render.
//
// Every judgment below MIRRORS the Rust that actually decides, and must keep
// mirroring it:
//   • infeasible  ⇐ ZERO open `- [ ]` steps AND zero checked `- [x]` steps
//     (ralphy-core `plan::count_open_steps`, read by runner/phases/plan.rs — a plan
//     whose steps are all checked was already executed, and the runner sends it
//     to the gates).
//   • reason      ⇐ the body under `^##\s+Feasible\b.*$` (core `handoff::infeasible_reason`;
//     the heading carries the verdict, so any tail after "Feasible" is accepted).
//   • needsSplit  ⇐ a heading other than `Feasible: yes` and that reason containing
//     the literal word "bundle" (core `handoff::is_bundle_verdict`).
export const FEASIBLE_HEAD = /^##\s+Feasible\b.*$/im;
// The `## Feasible` heading verbatim ("Feasible: no"), for the modal's banner.
export function feasibleHeading(md: string | null | undefined) {
  return (
    (md || "")
      .match(FEASIBLE_HEAD)?.[0]
      .replace(/^##\s+/, "")
      .trim() || ""
  );
}

// The planner's stated reason: the prose under that heading.
export function feasibleReason(md: string | null | undefined) {
  const head = feasibleHeading(md);
  return head ? section(md, head) : "";
}

export function isBundleReason(reason: string | null | undefined) {
  return (reason || "").toLowerCase().includes("bundle");
}

export const FEASIBLE_YES = /^##\s+Feasible\s*:\s*yes\b/im;
// One plan.md → what the board needs to know about it. `issue` is null for prose
// carrying no trailer, and the caller MUST treat that as "no plan for anyone":
// an unfinished plan is not yet a plan (see `planBelongsTo`).
export function planSummary(md: string | null | undefined) {
  const text = md || "";
  const steps = parseSteps(text);
  const openSteps = steps.filter((s) => s.status === "open").length;
  const checkedSteps = steps.filter((s) => s.status === "checked").length;
  const reason = feasibleReason(text);
  return {
    issue: planTrailerIssue(text),
    heading: feasibleHeading(text),
    reason,
    steps: steps.length,
    openSteps,
    // A plan with no step at all is the planner's refusal, whatever its
    // heading says — that is the same test the runner applies.
    infeasible: openSteps === 0 && checkedSteps === 0,
    needsSplit:
      openSteps === 0 && checkedSteps === 0 && !FEASIBLE_YES.test(text) && isBundleReason(reason),
  };
}

// The pill's words. `open` is whether the ISSUE is still open: a plan left over
// from a closed issue is not "ready", it is residue, and saying so is the
// difference between an invitation and a warning.
export function planPillLabel(summary: PlanVerdict | null | undefined, open: boolean = true) {
  if (!summary) return "";
  if (!open) return "leftover plan";
  if (summary.needsSplit) return "needs split";
  if (summary.infeasible) return "not feasible";
  return "plan ready";
}

// Whether the pill is a warning rather than an invitation — the one thing the
// operator must not have to open a modal to notice.
export function planPillWarns(summary: PlanVerdict | null | undefined, open: boolean = true) {
  return !!summary && (!open || summary.infeasible);
}
