/* ---------------------------------------------------------------------------
   The open project after a read of `/api/repos`. A project with no remote is
   keyed by a hash of its path; when it gets a remote the daemon re-keys it,
   and the new row lists the old key in `former_slugs`.
   --------------------------------------------------------------------------- */

import type { RepoRow } from "./wb-api.ts";
import { WBFleet } from "./wb-fleet.ts";
import type { Project } from "./wb-project.ts";

// The slug to keep open: the same slug when a local row still has it, the new
// slug of the row that was re-keyed from it, or null when no row has it. An
// open peer ref is kept: the peer rows arrive with the fleet read.
export function openAfterRead(
  openSlug: string | null,
  repos: RepoRow[],
  rows: Project[],
): string | null {
  if (openSlug === null) return null;
  const peer = rows.some((p) => (p.daemon ?? "") !== "" && WBFleet.repoRef(p) === openSlug);
  if (peer || repos.some((r) => r.slug === openSlug)) return openSlug;
  return repos.find((r) => r.former_slugs?.includes(openSlug) === true)?.slug ?? null;
}
