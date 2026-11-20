// The document worker: one request in, the model of the bytes or the reason
// they could not be read out (ADR-0091 D10). A failure is a reply, not a throw.
import initAnydoc, { toDocument } from "./vendor/anydoc/anydoc_wasm.js";
import type { ConvertErrorCode } from "./vendor/anydoc/anydoc_wasm.js";
import type { AnydocFailure, AnydocReply, AnydocRequest } from "./wb-anydoc.ts";
import { devError } from "./wb-log.ts";

const CONVERT_ERROR_CODES: readonly ConvertErrorCode[] = [
  "unsupported",
  "needsOcr",
  "malformed",
  "encrypted",
  "resourceLimit",
  "missingPart",
];

function isConvertErrorCode(code: unknown): code is ConvertErrorCode {
  return CONVERT_ERROR_CODES.some((known) => known === code);
}

/** The code anydoc put on its error, or `failed` for any other throw. */
export function failureOf(err: unknown): AnydocFailure {
  const code = err instanceof Error && "code" in err ? err.code : undefined;
  return isConvertErrorCode(code) ? code : "failed";
}

/** Read one request with anydoc, which must be initialized already. */
export function answerRequest(request: AnydocRequest): AnydocReply {
  try {
    return { id: request.id, ok: true, document: toDocument(request.bytes, request.format) };
  } catch (err) {
    return { id: request.id, ok: false, code: failureOf(err) };
  }
}

function failedReply(id: number): AnydocReply {
  return { id, ok: false, code: "failed" };
}

/** The part of the worker scope the worker uses. */
export interface AnydocWorkerScope {
  addEventListener(type: "message", listener: (event: MessageEvent<AnydocRequest>) => void): void;
  postMessage(reply: AnydocReply, options: StructuredSerializeOptions): void;
}

/** Answer each request of `scope` once `ready` has loaded the wasm. */
export function wireAnydocWorker(scope: AnydocWorkerScope, ready: () => Promise<unknown>): void {
  let loaded: Promise<boolean> | null = null;
  scope.addEventListener("message", (event) => {
    loaded ??= ready().then(
      () => true,
      (err: unknown) => {
        loaded = null;
        devError("[workbench] loading anydoc failed", err);
        return false;
      },
    );
    void loaded.then((ok) => {
      const request = event.data;
      const reply = ok ? answerRequest(request) : failedReply(request.id);
      scope.postMessage(reply, { transfer: [] });
      return reply;
    });
  });
}

// Only a worker scope has `importScripts`; a test imports this file in Node.
if ("importScripts" in globalThis) wireAnydocWorker(globalThis, () => initAnydoc());
