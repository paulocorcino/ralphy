// The "Run scripts" control of an HTML file pane (ADR-0089): the operator
// allows the scripts of one tab, and the file then comes from a route of the
// daemon with a token. Any new draw of the pane ends the permission.
import { apiFetch, commandReply } from "./wb-api.ts";
import { inPane } from "./wb-dom-need.ts";
import { WBFail } from "./wb-fail.ts";
import { mountFrame } from "./wb-viewer-html.ts";
import type { Messages } from "./wb-messages.ts";
import type { BytesRecord, ViewerRecord } from "./wb-file-viewer-records.ts";

/** What `createViewer` hands the control. */
export type ViewerScriptsDeps = {
  document: Document;
  messages: Pick<Messages, "flash">;
  /** Whether `rec` is still the record open under its id. */
  alive: (rec: ViewerRecord) => boolean;
  setCaption: (btn: HTMLElement, icon: string, caption: string) => void;
  /** Draw the pane again with no script. */
  renderStatic: (rec: BytesRecord) => void;
};

export type ViewerScripts = ReturnType<typeof createViewerScripts>;

// `file.preview`, sent on the command route itself and not through the
// daemon client of the page: the detached file window has no such client.
async function preview(payload: CommandPayload): Promise<ReplyOf<"file.preview">> {
  const res = await apiFetch("POST /api/command", {
    body: JSON.stringify({ id: 1, verb: "file.preview", payload }),
    headers: { "content-type": "application/json" },
    what: "file.preview",
  });
  if (!res.ok) throw new Error(`the command answered HTTP ${res.status}`);
  return commandReply<"file.preview">(res);
}

/** The address of a file of a preview, under its token (ADR-0089 D3). */
export function previewUrl(token: string, path: string): string {
  const segments = path.split("/").map((segment) => encodeURIComponent(segment));
  return `/preview/${token}/${segments.join("/")}`;
}

export function createViewerScripts(deps: ViewerScriptsDeps) {
  const { document, messages, alive, setCaption, renderStatic } = deps;

  function paint(rec: ViewerRecord) {
    const button = rec.el?.querySelector<HTMLElement>('[data-act="scripts"]');
    if (!button) return;
    const on = rec.scripts !== undefined;
    button.classList.toggle("on", on);
    setCaption(
      button,
      on ? "bi-stop-circle" : "bi-play-circle",
      on ? "Stop scripts" : "Run scripts",
    );
  }

  /** End the permission of the pane. The caller draws the pane again. */
  function end(rec: ViewerRecord) {
    const token = rec.scripts;
    if (token === undefined) return;
    rec.scripts = undefined;
    paint(rec);
    // The token also ends by itself, after a time with no request.
    preview({ repo: rec.project, revoke: token }).catch(() => {});
  }

  async function allow(rec: BytesRecord) {
    const scope: CommandPayload = { repo: rec.project };
    if (rec.checkout !== null && rec.checkout !== "") scope.checkout = rec.checkout;
    const reply = await preview(scope);
    const token = reply.token ?? "";
    if (WBFail.isError(reply) || token === "") {
      messages.flash(WBFail.failed(reply, "Could not run the scripts: the daemon gave no reason."));
      return;
    }
    if (!alive(rec) || rec.editing) {
      preview({ repo: rec.project, revoke: token }).catch(() => {});
      return;
    }
    rec.scripts = token;
    paint(rec);
    mountFrame(inPane(rec, ".md-body"), document, "allow-scripts").src = previewUrl(
      token,
      rec.path,
    );
  }

  /** The click on the control: allow the scripts, or go back to no script. */
  function toggle(rec: BytesRecord) {
    if (rec.scripts !== undefined) {
      renderStatic(rec);
      return;
    }
    // The route serves the file on disk, not the text of the editor.
    if (rec.dirty === true) {
      messages.flash("Save the file before you run its scripts.");
      return;
    }
    allow(rec).catch(() => {
      messages.flash("Could not run the scripts: the daemon did not answer.");
    });
  }

  return { toggle, end };
}
