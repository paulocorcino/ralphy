// The value wordings of the run text domain, read from the one catalog every
// surface reads (ADR-0078 D9). The build serves the catalog at
// `./wording/run.json`.
import run from "../../../../assets/wording/run.json" with { type: "json" };

type IssueStatuses = (typeof run.values)["issue-status"];
/** A status an issue has in a run, as the run snapshot names it. */
export type IssueStatusKey = keyof IssueStatuses;

/** The words and icon a surface shows for one value. */
export type ValueWording = { text: string; icon: string; fallback: string };

const ISSUE_STATUSES: IssueStatuses = run.values["issue-status"];

function isIssueStatus(raw: string): raw is IssueStatusKey {
  return Object.hasOwn(ISSUE_STATUSES, raw);
}

/** The key of the status `raw` names, or null for a name the catalog lacks. */
export function issueStatus(raw: string): IssueStatusKey | null {
  return isIssueStatus(raw) ? raw : null;
}

/** The catalog wording of one issue status. */
export function statusWording(key: IssueStatusKey): ValueWording {
  const { text, icon, fallback } = ISSUE_STATUSES[key];
  return { text, icon, fallback };
}
