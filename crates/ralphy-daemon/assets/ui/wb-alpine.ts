// An Alpine component's data, with its `uses` list (ADR-0073 D4) turned into
// the type of its `this` (ADR-0075 D6): the component's own members, the
// `shell()` members it lists, and the Alpine magics. Reading another `shell()`
// member is a type error. The markup is not type-checked, so the harness's
// `loadComponent` still checks the names the markup reads.
import type { Shell } from "./app.ts";

/** The Alpine magics a component or `shell()` calls. */
export interface AlpineMagics {
  $nextTick(callback?: () => void): Promise<void>;
  $refs: Record<string, HTMLElement | undefined>;
}

export function component<U extends keyof Shell, T extends object>(
  uses: readonly U[],
  data: T & ThisType<T & Pick<Shell, U> & AlpineMagics>,
): T & { uses: U[] } {
  return Object.assign(data, { uses: [...uses] });
}
