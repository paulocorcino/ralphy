/* ---------------------------------------------------------------------------
   The modal stack of this page: the Alpine store `modals` (#621; ADR-0073 D6
   and the amendment of 2026-10-08, decisions 3 and 6).

   The store is the one owner of the open modals and their order, the confirm
   and prompt dialogs, and the flash line. Its fields change only through its
   own methods. `shell()` code reads `this.$store.modals`, the markup reads
   `$store.modals`, a component reads it through the `$store` magic, and code
   outside Alpine reads `Alpine.store("modals")`.

   Every `.modal-scrim` in index.html takes its behavior from `scrim()`. The
   open flag it names is read on the scope of the scrim, the `this` Alpine
   gives each function of the binding, so a component keeps its own flag.

   It reads no `shell()` member and calls no component; it reads the store
   `connection`. It never calls the door for operator messages
   (wb-messages.ts): the door's `flash` and `askInShell` end here, so a call
   back would loop.

   `main.ts` registers it as `modals`, first of the stores. `modalsStore`
   builds it without Alpine, so `node --test` can test it (ADR-0073 D3, D6).
   --------------------------------------------------------------------------- */
import { WBReleaseDialogs } from "./wb-release-dialogs.ts";
import { WBSecurityDialog } from "./wb-security-dialog.ts";
import { WBSettingsDialog } from "./wb-settings-dialog.ts";
import type { Messages } from "./wb-messages.ts";
import type { ConfirmAsk, ModalEntry, PromptAsk, Timer } from "./wb-shell-types.d.ts";

/** What `main.ts` gives the store. */
export type ModalsStoreDeps = {
  /** The door for operator messages. */
  messages: Messages;
};

// Whether the focus is on an element that takes the keys: a field, an editable
// element or the code editor.
function focusTakesKeys(el: HTMLElement | null) {
  return !!(
    el &&
    (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable || el.closest(".monaco-editor"))
  );
}

// Whether the Settings, Security, Run or Branch dialog is on the modal stack.
function blockingDialogOpen(modals: { modalOpen(path: string): boolean }) {
  return (
    modals.modalOpen(WBSettingsDialog.openFlag) ||
    modals.modalOpen(WBSecurityDialog.openFlag) ||
    modals.modalOpen("runOpen") ||
    modals.modalOpen("branchOpen")
  );
}

// `deps` is the shape every store takes; the store is where the door's flash
// and confirm end, so it never calls the door.
export function modalsStore(_deps: ModalsStoreDeps) {
  return {
    // The flash line; `flash` writes it, and it clears itself.
    runsActionMsg: "",
    _actionTimer: null as Timer,
    // The message door (`wb-messages.ts`), `shell()` and the components flash here.
    flash(msg: string) {
      this.runsActionMsg = msg;
      clearTimeout(this._actionTimer);
      this._actionTimer = setTimeout(() => (this.runsActionMsg = ""), 2600);
    },

    // --- modal stack ------------------------------------------------------
    // The open modals, oldest first: `{ path, opener }`. Only the last one
    // answers Escape, and each returns focus to its opener on close.
    _modalStack: [] as ModalEntry[],
    // The confirm dialog (replaces window.confirm); `askConfirm` opens it.
    confirmModal: {
      open: false,
      title: "",
      message: "",
      confirmLabel: "Confirm",
      cancelLabel: "Cancel",
      danger: false,
    },
    _confirmResolve: null as ((ok: boolean) => void) | null,
    // The prompt dialog (replaces window.prompt, which is suppressible
    // per-origin and never appears in an unfocused popup). `askPrompt`
    // resolves the typed string, or null.
    promptModal: {
      open: false,
      title: "",
      message: "",
      value: "",
      placeholder: "",
      confirmLabel: "Create",
      error: "",
    },
    _promptResolve: null as ((name: string | null) => void) | null,
    // The Escape keydown a modal has already answered.
    _escapeEvent: null as Event | null,
    // The one binding every `.modal-scrim` in index.html uses:
    // `x-bind="$store.modals.scrim('runOpen', () => closeRunModal())"`. `path`
    // names the open flag on the scrim's scope, dotted for a nested one
    // (`$store.modals.confirmModal.open`). Alpine evaluates the object once per
    // scrim, so `was` lives as long as the element, and runs each function
    // with the scrim's scope as `this`.
    // A click on the scrim closes nothing: a stray click must not throw away
    // what a modal holds. Only its own buttons and Escape close it.
    scrim(path: string, close: () => void) {
      const isOpen = (scope: object) => path.split(".").reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], scope);
      let was = false;
      const self = this;
      return {
        "x-show"(this: object) {
          return isOpen(this);
        },
        // Every open scrim hears the same window keydown; only the top one acts,
        // so a confirm raised over another modal closes alone. The event is
        // marked because the browser runs Alpine's effects between two
        // listeners: the close pops the stack before the next scrim is asked,
        // and the modal under it would read as the top.
        "@keydown.escape.window"(this: object, e: Event) {
          if (self._escapeEvent === e) return;
          if (isOpen(this) && self.isTopModal(path)) {
            self._escapeEvent = e;
            close();
          }
        },
        // Watches the flag, not the close methods: `logOff()` clears flags
        // directly, and that close must still pop the stack.
        "x-effect"(this: { $el: Element }) {
          const open = !!isOpen(this);
          if (open === was) return;
          was = open;
          if (open) self.modalOpened(path, this.$el);
          else self.modalClosed(path);
        },
      };
    },
    isTopModal(path: string) {
      return this._modalStack.at(-1)?.path === path;
    },
    // Whether the modal with this open-flag path is open, at any depth. Code
    // outside a dialog's component asks this, never the flag (ADR-0073 D5).
    modalOpen(path: string) {
      return this._modalStack.some((m) => m.path === path);
    },
    modalOpened(path: string, scrimEl: Element) {
      this._modalStack.push({ path, opener: document.activeElement as HTMLElement | null });
      // One frame later: `x-show` has flipped by then, and a modal that focuses
      // its own field on open (Branch, Prompt) has already done so.
      window.requestAnimationFrame(() => {
        const dialog = scrimEl.querySelector('[role="dialog"], [role="alertdialog"]') || scrimEl;
        if (dialog.contains(document.activeElement)) return;
        const controls = Array.from(
          dialog.querySelectorAll<HTMLElement & { disabled?: boolean }>(
            'button, input, select, textarea, [href], [tabindex]:not([tabindex="-1"])',
          ),
        ).filter((el) => !el.disabled && el.getClientRects().length > 0);
        // The header ✕ is the last resort: it leads every modal, and the
        // operator came for the content.
        (controls.find((el) => !el.classList.contains("modal-x")) || controls[0])?.focus();
      });
    },
    modalClosed(path: string) {
      const i = this._modalStack.findLastIndex((m) => m.path === path);
      if (i < 0) return;
      const [{ opener }] = this._modalStack.splice(i, 1);
      if (opener?.isConnected) opener.focus();
    },
    // Accelerators are ignored while typing or while a modal is up.
    // `allowTerminal`: a key that must also work from inside a terminal (the
    // column walk); xterm's input is a TEXTAREA.
    consoleShortcutsBlocked(allowTerminal = false) {
      if (!window.Alpine.store("connection").authed) return true;
      if (blockingDialogOpen(this)) return true;
      if (this.modalOpen(WBReleaseDialogs.whatsNewFlag)) return true;
      const el = document.activeElement as HTMLElement | null;
      if (allowTerminal && el?.closest?.(".xterm")) return false;
      return focusTakesKeys(el);
    },
    // Resolve `true`/`false` on the operator's choice. A pending dialog is
    // settled `false` first so a second call never strands its promise.
    askConfirm(opts: ConfirmAsk = {}) {
      if (this._confirmResolve) this.confirmRespond(false);
      this.confirmModal = {
        open: true,
        title: opts.title || "Confirm",
        message: opts.message || "",
        confirmLabel: opts.confirmLabel || "Confirm",
        cancelLabel: opts.cancelLabel || "Cancel",
        danger: opts.danger || false,
      };
      return new Promise((resolve) => {
        this._confirmResolve = resolve;
      });
    },
    // Close the dialog and settle its promise with the choice.
    confirmRespond(ok: boolean) {
      this.confirmModal.open = false;
      const resolve = this._confirmResolve;
      this._confirmResolve = null;
      if (resolve) resolve(ok);
    },
    // Resolve the typed string, or `null`. Mirrors askConfirm.
    askPrompt(opts: PromptAsk = {}) {
      if (this._promptResolve) this.promptRespond(null);
      this.promptModal = {
        open: true,
        title: opts.title || "Name",
        message: opts.message || "",
        value: opts.value || "",
        placeholder: opts.placeholder || "",
        confirmLabel: opts.confirmLabel || "Create",
        error: "",
      };
      // Focus after Alpine has painted; caret at the end, not selected.
      queueMicrotask(() => {
        const el = document.getElementById("prompt-input") as HTMLInputElement | null;
        if (!el) return;
        el.focus();
        el.setSelectionRange(el.value.length, el.value.length);
      });
      return new Promise<string | null>((resolve) => {
        this._promptResolve = resolve;
      });
    },
    // The prompt's field changed: keep the text, and clear the refusal under
    // it. The markup calls this, so the store stays the one writer of its
    // fields.
    promptTyped(value: string) {
      this.promptModal.value = value;
      this.promptModal.error = "";
    },
    // A name that cannot be a single directory entry is refused HERE, dialog
    // open. The daemon confines every path regardless (`confine_write`); this
    // says *which* character was wrong.
    promptSubmit() {
      const name = this.promptModal.value.trim();
      const bad = !name
        ? "name is required"
        : /[\\/]/.test(name)
          ? "name cannot contain / or \\"
          : name === "." || name === ".."
            ? "name cannot be . or .."
            : "";
      if (bad) {
        this.promptModal.error = bad;
        return;
      }
      this.promptRespond(name);
    },
    // Close the dialog and settle its promise with `name` (null = cancelled).
    promptRespond(name: string | null) {
      this.promptModal.open = false;
      const resolve = this._promptResolve;
      this._promptResolve = null;
      if (resolve) resolve(name);
    },
  };
}

/** The Alpine store `modals`. */
export type ModalsStore = ReturnType<typeof modalsStore>;
