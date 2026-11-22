// A vendored library of a viewer loads the first time a file of its kind
// opens, never at page start (ADR-0091 D8).
import { devError } from "./wb-log.ts";

/** The outcome of a load: the module, or the text the viewer shows instead. */
export type VendorLoad = { ok: true; module: unknown } | { ok: false; text: string };

/** Imports one served path; the page passes `import()`, a test a fake. */
export type VendorImport = (path: string) => Promise<unknown>;

export const VENDOR_FAILURE_TEXT =
  "The viewer for this file could not load. Reload the tab to try again.";

/**
 * A loader that imports each path once and shares the promise. A failed
 * import is forgotten, so the next open of the tab tries again.
 */
export function createVendorLoader(importer: VendorImport): (path: string) => Promise<VendorLoad> {
  const loads = new Map<string, Promise<VendorLoad>>();
  return (path) => {
    const known = loads.get(path);
    if (known) return known;
    const load = importer(path).then(
      (module): VendorLoad => ({ ok: true, module }),
      (err: unknown): VendorLoad => {
        loads.delete(path);
        devError(`[workbench] loading ${path} failed`, err);
        return { ok: false, text: VENDOR_FAILURE_TEXT };
      },
    );
    loads.set(path, load);
    return load;
  };
}

/** Load a vendored module by its path under `./vendor/`, once per page. */
export const loadVendor = createVendorLoader((path) => import(path));
