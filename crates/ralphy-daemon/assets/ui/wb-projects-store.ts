/* ---------------------------------------------------------------------------
   The open project and the list of projects: the Alpine store `projects`
   (ADR-0073 amendment of 2026-10-08, decision 3).

   The store is the one owner of this fact. Its fields change only through
   `setOpen` and `setProjects`. `shell()` code reads `this.$store.projects`,
   the markup reads `$store.projects`, a component reads it through the
   `$store` magic, and code outside Alpine reads `Alpine.store("projects")`.
   A helper that needs other `shell()` state too stays in `shell()`.

   `main.ts` registers it as `projects`. `projectsStore` builds it without
   Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { WBFleet } from "./wb-fleet.ts";
import { WBProject } from "./wb-project.ts";
import type { Project } from "./wb-project.ts";

export function projectsStore() {
  return {
    // The ref of the open project (`repoRef`), or null.
    openSlug: null as string | null,
    // `loadRepos()` fills this at init.
    projects: [] as Project[],
    setOpen(slug: string | null) {
      this.openSlug = slug;
    },
    setProjects(list: Project[]) {
      this.projects = list;
    },
    repoRef(p: Project) {
      return WBFleet.repoRef(p);
    },
    // The open project's row, for the panels scoped to `openSlug` that reuse a
    // per-row control (the Changes head's checkout chip).
    openProject() {
      return this.projects.find((p) => this.repoRef(p) === this.openSlug) || null;
    },
    // The current git branch of the open project (for the "current" mode blurb).
    openProjectBranch() {
      return this.projects.find((p) => this.repoRef(p) === this.openSlug)?.branch || "current";
    },
    // The branch chip lives on the Files bar (#332), which only the OPEN
    // project renders. `.project-slug` carries the ADR-0008 D7 identity in
    // `data-slug`, which is how the browser tests find a row.
    rowOpen(p: Project) {
      return this.openSlug === this.repoRef(p);
    },
    // What every surface OUTSIDE the sidebar prints for a repo ref. A peer ref
    // is `<daemon_id>/<owner>/<repo>`: the ULID is how the fleet ROUTES
    // (ADR-0052 §5), so the environment is printed in its place. The ref itself
    // is untouched on the wire, the desk and the tab ids. Row lookup by
    // `repoRef`, not slug: the same `owner/repo` on two daemons is two rows.
    projectLabel(ref: string | null | undefined) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return WBFleet.refLabel(ref);
      const name = WBProject.projectName(row);
      return row.daemon && row.env ? `${name} · ${row.env}` : name;
    },
    // The tooltip twin of `projectLabel`: `owner/repo`, or the full folder of
    // a remoteless repo, plus the environment of a peer. Never a hash key or
    // a daemon id.
    projectTitle(ref: string | null | undefined) {
      if (!ref) return "";
      const row = this.projects.find((p) => this.repoRef(p) === ref);
      if (!row) return WBFleet.refLabel(ref);
      const who = WBProject.projectTitle(row);
      return row.daemon && row.env ? `${who} · ${row.env}` : who;
    },
  };
}

/** The Alpine store `projects`. */
export type ProjectsStore = ReturnType<typeof projectsStore>;
