/* ---------------------------------------------------------------------------
   ralphy workbench — the one door for operator messages

   An operator message (CONTEXT.md) is a confirm, a notice, a toast or a
   flash. Every module outside `shell()` tells the operator through this door
   (ADR-0073, the 2026-10-10 amendment, decision 5). It has two drawings: the
   dialog and the toast below are DOM-built, so they work in every page, the
   detached-fence popup too, which has no shell; the shell's Alpine modal and
   its flash line are reached through `shell`, and only when the shell
   exists. An Alpine component nested in `shell()` lists the shell's `flash`
   and `askConfirm` in its `uses`: it exists only where the shell does, and
   it is built with no arguments.

   Importing this module does nothing. Each entry module calls
   `createMessages` once per page, before the console and the note cards
   (ADR-0075 D7).
--------------------------------------------------------------------------- */
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { ConfirmAsk } from "./wb-shell-types.d.ts";

/**
 * The two members of `shell()` (`app.ts`) the door calls. Written out, not
 * picked from the shell's type: that type is inferred from `shell()`, which
 * reads this module's type through `window.WBConsole`, whose options take
 * the door, and tsc 5.9.3 then loses the type of `this` inside `shell()`
 * (measured).
 */
export type ShellMessages = {
  askConfirm(opts: ConfirmAsk): Promise<unknown>;
  flash(msg: string): void;
};

/** What the entry module hands the door. */
export type MessagesDeps = {
  /** The page's shell, read at each call; `null` in a page that has none. */
  shell: () => ShellMessages | null;
};

/** The door one page builds. */
export type Messages = ReturnType<typeof createMessages>;

export function createMessages(window: Pick<Window, "setTimeout" | "clearTimeout">, document: Document, deps: MessagesDeps) {
  const { shell } = deps;

  // Ask before a click that cannot be taken back: tiling moves every console in
  // a fence, removing a fence takes the region out from under them, a console's
  // × ends a live session — each one pixel from something harmless.
  //
  // Not the shell's Alpine dialog: this module also runs in the detached-fence
  // popup, which has no Alpine and no modal markup. It borrows the shell's
  // CLASSES (styles.css is loaded in both). Not `window.confirm`: it blocks the
  // thread, and an automated browser dismisses it by default — every guarded
  // click would silently cancel.
  // `notice: true` is the one-button form: OK alone, focused, Enter/Escape dismiss.
  function askConfirm({ title, message, confirmLabel = "Confirm", danger = false, notice = false }: ConfirmOptions) {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-confirm";
    const modal = document.createElement("div");
    modal.className = "modal confirm-modal";
    modal.setAttribute("role", "alertdialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", title);
    const head = document.createElement("div");
    head.className = "modal-head";
    const mark = document.createElement("i");
    mark.className = "bi " + (danger ? "bi-exclamation-triangle" : "bi-question-circle");
    if (danger) mark.style.color = "var(--danger)";
    head.append(mark);
    const heading = document.createElement("span");
    heading.className = "modal-title";
    heading.textContent = title;
    head.append(heading);
    const body = document.createElement("p");
    body.className = "confirm-body";
    body.textContent = message;
    const foot = document.createElement("div");
    foot.className = "modal-foot";
    const cancel = document.createElement("button");
    cancel.className = "btn";
    cancel.type = "button";
    cancel.textContent = "Cancel";
    const go = document.createElement("button");
    go.className = danger ? "btn danger" : "btn accent";
    go.type = "button";
    go.textContent = confirmLabel;
    if (notice) foot.append(go);
    else foot.append(cancel, go);
    modal.append(head, body, foot);
    scrim.append(modal);
    document.body.append(scrim);
    // CANCEL takes the keyboard: the dialog exists because a click went astray,
    // and a destructive button under a stray Enter would repeat the slip. A
    // notice has nothing to protect: OK takes the focus.
    if (notice) go.focus();
    else cancel.focus();

    return new Promise<boolean>((resolve) => {
      let settled = false;
      const done = (ok: boolean) => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        scrim.remove();
        resolve(ok);
      };
      const onKey = (e: KeyboardEvent) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          done(false);
        } else if (e.key === "Enter" && document.activeElement === go) {
          e.stopPropagation();
          done(true);
        }
      };
      // CAPTURE: a console's terminal swallows keystrokes, and Escape is one it
      // forwards to the child — the dialog must hear it first.
      document.addEventListener("keydown", onKey, true);
      cancel.addEventListener("click", () => done(false));
      go.addEventListener("click", () => done(true));
    });
  }

  // A message with an OK and nothing else (a verb's refusal, verbatim).
  function askNotice({ title, message, danger = true }: { title: string; message: string; danger?: boolean }) {
    return askConfirm({ title, message, confirmLabel: "OK", danger, notice: true });
  }

  // A transient line with ONE optional verb — the undo a close needs (ADR-0064
  // §11). Not a dialog: it asks nothing, takes no focus and never blocks the
  // stage, because the act it reports already happened and the file it reports
  // on is still there. One at a time: a second replaces the first, so a burst
  // of closes cannot stack a column over the plane.
  //
  // DOM-built like `askConfirm` and appended to `document.body`, so it works in
  // the detached-fence popup too, which has no shell around it.
  let toastEl: HTMLElement | null = null;
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  const TOAST_MS = 6000;
  function toast({ text, action, onAction, ms = TOAST_MS }: { text: string; action?: string; onAction?: () => void; ms?: number }) {
    dismissToast();
    const el = document.createElement("div");
    el.className = "wb-toast";
    el.setAttribute("role", "status");
    const line = document.createElement("span");
    line.className = "wb-toast-text";
    line.textContent = text;
    el.append(line);
    if (action && onAction) {
      const btn = document.createElement("button");
      btn.className = "wb-toast-action";
      btn.type = "button";
      btn.textContent = action;
      btn.addEventListener("click", () => {
        dismissToast();
        onAction();
      });
      el.append(btn);
    }
    const close = document.createElement("button");
    close.className = "wb-toast-close";
    close.type = "button";
    close.title = "Dismiss";
    close.textContent = "×";
    close.addEventListener("click", dismissToast);
    el.append(close);
    document.body.append(el);
    toastEl = el;
    toastTimer = window.setTimeout(dismissToast, ms);
    return el;
  }
  function dismissToast() {
    if (toastTimer != null) window.clearTimeout(toastTimer);
    toastTimer = null;
    toastEl?.remove();
    toastEl = null;
  }

  // The shell's own confirm, the Alpine modal of `index.html`. `null` where
  // the page has no shell: the caller decides what that page does instead.
  function askInShell(opts: ConfirmAsk) {
    const sh = shell();
    return sh ? sh.askConfirm(opts) : null;
  }

  // The shell's flash line. A page with no shell has no line, so it shows
  // nothing.
  function flash(text: string) {
    shell()?.flash(text);
  }

  return { askConfirm, askNotice, toast, dismissToast, askInShell, flash };
}
