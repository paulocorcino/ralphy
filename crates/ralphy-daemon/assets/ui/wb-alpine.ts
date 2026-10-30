// An Alpine component's data, with its `uses` list (ADR-0073 D4) turned into
// the type of its `this` (ADR-0075 D6): the component's own members, the
// `shell()` members it lists, and the Alpine magics. Reading another `shell()`
// member is a type error. The markup is not type-checked, so the harness's
// `loadComponent` still checks the names the markup reads.
import type { Shell } from "./app.ts";
import type { ProjectsStore } from "./wb-projects-store.ts";
import type { RunsStore } from "./wb-runs-store.ts";
import type { BoardStore } from "./wb-board-store.ts";
import type { CheckoutsStore } from "./wb-checkouts-store.ts";
import type { ConnectionStore } from "./wb-connection-store.ts";
import type { ReleaseStore } from "./wb-release-store.ts";
import type { ModalsStore } from "./wb-modals-store.ts";
import type { TabsStore } from "./wb-tabs-store.ts";
import type { UploadsStore } from "./wb-uploads-store.ts";

/** The Alpine magics a component or `shell()` calls. */
export interface AlpineMagics {
  $nextTick(callback?: () => void): Promise<void>;
  $refs: Record<string, HTMLElement | undefined>;
  $store: { projects: ProjectsStore; connection: ConnectionStore; runs: RunsStore; board: BoardStore; checkouts: CheckoutsStore; release: ReleaseStore; modals: ModalsStore; tabs: TabsStore; uploads: UploadsStore };
}

/** An empty map from a string key to a `V`, for a component's data. A cast
 * (`{} as Record<string, V>`) there is flagged as unneeded by the type-aware
 * lint, because the data object is its own contextual type. */
export function emptyRecord<V>(): Record<string, V> {
  return {};
}

export function component<U extends keyof Shell, T extends object>(
  uses: readonly U[],
  data: T & ThisType<T & Pick<Shell, U> & AlpineMagics>,
): T & { uses: U[] } {
  return Object.assign(data, { uses: [...uses] });
}
