// The page side of the document reader: docx, odt, xlsx and ods bytes go to
// anydoc-wasm in one module worker, and its model comes back (ADR-0091 D10).
import type { ConvertErrorCode, Document } from "./vendor/anydoc/anydoc_wasm.js";
import { devError } from "./wb-log.ts";

/** The kinds of file the worker reads. */
export type AnydocFormat = "docx" | "odt" | "xlsx" | "ods";

/** Why a read failed: the code anydoc gives, or `failed` for anything else. */
export type AnydocFailure = ConvertErrorCode | "failed";

/** The outcome of one read: the model, or the reason it failed. */
export type AnydocResult = { ok: true; document: Document } | { ok: false; code: AnydocFailure };

/** The one message the page sends the worker. */
export type AnydocRequest = { id: number; bytes: Uint8Array; format: AnydocFormat };

/** The one message the worker sends back, for the request of the same `id`. */
export type AnydocReply = { id: number } & AnydocResult;

/** The part of a `Worker` the reader uses; a test passes a fake. */
export interface AnydocWorker {
  addEventListener(type: "message", listener: (event: MessageEvent<AnydocReply>) => void): void;
  addEventListener(type: "error", listener: (event: ErrorEvent) => void): void;
  postMessage(request: AnydocRequest, options: StructuredSerializeOptions): void;
  terminate(): void;
}

/** How long the worker stays after its last answer, so its wasm memory goes. */
export const ANYDOC_IDLE_MS = 30_000;

/**
 * A reader that starts the worker on the first request, answers each request
 * with its own promise, and ends the worker once no request has waited for
 * `idleMs`. A crash of the worker answers every waiting request with a failure.
 */
export function createAnydocReader(
  spawn: () => AnydocWorker,
  idleMs: number = ANYDOC_IDLE_MS,
): (bytes: Uint8Array, format: AnydocFormat) => Promise<AnydocResult> {
  const waiting = new Map<number, (result: AnydocResult) => void>();
  let worker: AnydocWorker | null = null;
  let idle: ReturnType<typeof setTimeout> | undefined;
  let next = 0;

  const settle = (id: number, result: AnydocResult): void => {
    const resolve = waiting.get(id);
    waiting.delete(id);
    resolve?.(result);
    if (waiting.size === 0) idle = setTimeout(stop, idleMs);
  };
  const stop = (): void => {
    worker?.terminate();
    worker = null;
  };
  const crash = (event: ErrorEvent): void => {
    devError("[workbench] the document worker failed", event);
    stop();
    for (const id of [...waiting.keys()]) settle(id, { ok: false, code: "failed" });
  };
  const start = (): AnydocWorker => {
    const started = spawn();
    started.addEventListener("message", (event) => {
      const { id, ...result } = event.data;
      settle(id, result);
    });
    started.addEventListener("error", crash);
    return started;
  };

  return (bytes, format) => {
    clearTimeout(idle);
    worker ??= start();
    const id = next++;
    const answer = new Promise<AnydocResult>((resolve) => {
      waiting.set(id, resolve);
    });
    worker.postMessage({ id, bytes, format }, { transfer: [] });
    return answer;
  };
}

/** Read a document's bytes into its model, in the page's one document worker. */
export const readAnydoc = createAnydocReader(
  () => new Worker(new URL("./wb-anydoc-worker.js", import.meta.url), { type: "module" }),
);
