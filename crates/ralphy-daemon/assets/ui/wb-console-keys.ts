/* ---------------------------------------------------------------------------
   The console's input: the key bar, the font size and the clipboard.

   `createConsoleKeys(deps)` returns the input functions the console shares
   with the terminal (`fontSize`, `writeClipboard`, `readClipboard`) and
   `buildKeyBar(win)`, which builds the key bar of one window. It reads the
   console only through `deps`. The folds it uses are `wb-console-input.ts`
   (#621).
--------------------------------------------------------------------------- */
import * as WBConsoleInput from "./wb-console-input.ts";
import { touchInput } from "./wb-viewport.ts";
import type { WBView } from "./wb-client-view.ts";
import type { TerminalDeps } from "./wb-console-terminal.ts";
import type { ConsoleTerm, ConsoleWin } from "./wb-types.d.ts";

const { keyBarVisible, terminalInputMode, pasteOffered, pasteAfterRead, stepFont, clipboardContent, FONT_DEFAULT } = WBConsoleInput;

// What the key bar and the clipboard read from the console, and nothing else.
export type ConsoleKeysDeps = {
  // The console's page: the touch query. The popup's window answers it too.
  window: Window;
  // The page the key bar's elements and the clipboard's fallback are made in.
  document: Document;
  // The per-client view store (`WBView`): the key-bar setting and the font
  // size. The popup's store reads nothing.
  viewStore: Pick<typeof WBView, "read" | "patch">;
  // Every console window: a font change reaches each terminal.
  wins: Set<ConsoleWin>;
  // Raises and focuses a window.
  focusWin: (el: HTMLElement) => void;
};

export function createConsoleKeys(deps: ConsoleKeysDeps) {
  const { window, document, viewStore, wins, focusWin } = deps;


  // Per browser profile (wb-settings.ts `scope: client`, stored by
  // `wb-client-view.ts`). Absent — the popup reads nothing — means auto.
  function keyBarMode() {
    return viewStore?.read()?.keys ?? null;
  }

  function applyKeyBar(win: ConsoleWin) {
    win.classList.toggle("keys", keyBarVisible(keyBarMode(), touchInput(window)));
    win._applyInputMode?.();
  }

  function fontSize() {
    return viewStore?.read()?.font ?? FONT_DEFAULT;
  }

  // Every window at once. `fit` is required: the BOX does not change, so the
  // ResizeObserver never fires and the daemon would never learn the new size.
  function setFont(px: number) {
    viewStore?.patch({ font: px });
    for (const w of wins) {
      const t = w._term;
      if (!t) continue;
      t.term.options.fontSize = px;
      try {
        t.fit.fit();
      } catch {}
    }
    return px;
  }

  // Put `text` on the system clipboard and give the terminal its focus back.
  // `navigator.clipboard` exists only in a SECURE CONTEXT (loopback, https), so
  // the write degrades to a hidden textarea + `execCommand`. Every path is
  // best-effort and SILENT: `writeText` rejects with "Document is not focused"
  // whenever the workbench is not the focused window, and Chrome can refuse
  // `execCommand` outside a user gesture; the copy is dropped, not queued.
  function writeClipboard(text: string, term: XtermTerminal) {
    if (!text) return;
    // The fallback moves focus to the textarea; without giving it back, the
    // operator's next keystroke goes nowhere and the console looks dead.
    const fallback = () => {
      // A detached popup has its OWN document — write into the one this terminal
      // actually lives in, not the shell's.
      const doc = term?.element?.ownerDocument || document;
      const ta = doc.createElement("textarea");
      ta.value = text;
      // Off-screen rather than `display:none`: a hidden element cannot be selected.
      ta.style.position = "fixed";
      ta.style.left = "-9999px";
      doc.body.append(ta);
      try {
        ta.select();
        doc.execCommand("copy");
      } catch {}
      ta.remove();
      try {
        term?.focus();
      } catch {}
    };
    if (!navigator.clipboard) {
      fallback();
      return;
    }
    navigator.clipboard.writeText(text).catch(fallback);
  }

  // The read half: resolves `{ image: Blob }` or `{ text }`. Always a promise:
  // an insecure origin has no `navigator.clipboard` and the API throws
  // synchronously. `read()` first, because `readText()` resolves "" for an
  // image-only clipboard (an iOS screenshot). ONE call: each read raises
  // Safari's "Paste" callout, so a text fallback would ask twice.
  function readClipboard(): ReturnType<TerminalDeps["readClipboard"]> {
    try {
      const clip = navigator.clipboard;
      if (typeof clip.read !== "function") {
        return Promise.resolve(clip.readText()).then((text) => ({ text }));
      }
      return Promise.resolve(clip.read()).then(clipboardContent);
    } catch {
      return Promise.resolve({ text: "" });
    }
  }

  // The key bar of one window. Its latch buttons go back to the caller, which
  // reflects the terminal's latches on them.
  function buildKeyBar(win: ConsoleWin) {
    // THE KEY BAR. Here, not in `buildChrome`: a placeholder has no session
    // for the buttons to talk to.
    const bar = document.createElement("div");
    bar.className = "session-keys";
    // The strip refuses focus: on iOS losing the textarea's caret dismisses
    // the keyboard. `pointerdown` + `mousedown`; `touchstart` is NOT
    // prevented — that would suppress the synthesized click.
    const holdFocus = (e: Event) => e.preventDefault();
    bar.addEventListener("pointerdown", holdFocus);
    bar.addEventListener("mousedown", holdFocus);

    // `icon` (a Bootstrap Icons class) draws the key as that glyph; without
    // it the key shows `text`.
    const key = (name: string, text: string, title: string, cls?: string, icon?: string) => {
      const b = document.createElement("button");
      b.type = "button";
      // Not in the tab order: a keyboard user already has these keys.
      b.tabIndex = -1;
      b.className = "session-key" + (cls ? " " + cls : "");
      b.dataset.key = name;
      b.title = title;
      if (icon) {
        const i = document.createElement("i");
        i.className = "bi " + icon;
        b.append(i);
      } else {
        b.textContent = text;
      }
      bar.append(b);
      return b;
    };

    // The virtual keyboard opens only from here (`terminalInputMode`). Any
    // blur of the field closes it: the phone's own Done key, another window
    // taking focus, the page going to the background.
    let keyboardOpen = false;
    let refocusing = false;
    const kbBtn = key("keyboard", "", "Show or hide the keyboard", "", "bi-keyboard");
    kbBtn.setAttribute("aria-pressed", "false");
    const applyInputMode = () => {
      kbBtn.setAttribute("aria-pressed", keyboardOpen ? "true" : "false");
      const field = win._term?.term.textarea;
      if (!field) return;
      const mode = terminalInputMode(win.classList.contains("keys"), keyboardOpen);
      if (mode) field.setAttribute("inputmode", mode);
      else field.removeAttribute("inputmode");
    };
    win._applyInputMode = applyInputMode;
    const toggleKeyboard = (field: HTMLTextAreaElement) => {
      keyboardOpen = !keyboardOpen;
      applyInputMode();
      // iOS reads `inputmode` only when the field takes focus. The click
      // handler focuses it again, inside the same tap.
      refocusing = true;
      field?.blur();
      refocusing = false;
    };

    // Most used first: on a phone only the keys up to Down show without a
    // scroll (the narrow-screen rule in 06-consoles.css).
    key("esc", "esc", "Escape");
    const shiftBtn = key("shift", "shift", "Shift: applies to the next key");
    shiftBtn.setAttribute("aria-pressed", "false");
    key("tab", "tab", "Tab");
    key("enter", "", "Enter", "", "bi-arrow-return-left");
    key("slash", "/", "Slash");
    key("up", "", "Up", "", "bi-arrow-up");
    key("down", "", "Down", "", "bi-arrow-down");
    key("left", "", "Left", "", "bi-arrow-left");
    key("right", "", "Right", "", "bi-arrow-right");
    const ctrlBtn = key("ctrl", "ctrl", "Ctrl: applies to the next key");
    ctrlBtn.setAttribute("aria-pressed", "false");
    key("ctrl-c", "^C", "Ctrl-C: interrupt");
    // Arms ONE drag to select whole lines; the gesture's end disarms it.
    const selBtn = key("select", "sel", "Select lines: drag across the screen");
    selBtn.setAttribute("aria-pressed", "false");

    const gap = document.createElement("span");
    gap.className = "session-keys-gap";
    bar.append(gap);

    // `writeClipboard`'s textarea fallback runs inside this click (a user
    // gesture), which is what makes it work on an insecure LAN origin.
    // `bi-copy`, not `bi-clipboard`: the clipboard glyph is the PASTE icon.
    const copyBtn = key("copy", "", "Copy selection", "", "bi-copy");
    copyBtn.disabled = true;
    const syncCopy = () => {
      copyBtn.disabled = !win._term?.term.hasSelection();
    };
    // The one piece of chrome bound to a PARTICULAR terminal:
    // `onSelectionChange` is on the xterm instance, so a woken console's new
    // instance needs it again.
    win._rewire = (t: ConsoleTerm) => {
      t.term.onSelectionChange(syncCopy);
      syncCopy();
      t.term.textarea?.addEventListener("blur", () => {
        if (refocusing || !keyboardOpen) return;
        keyboardOpen = false;
        applyInputMode();
      });
      applyInputMode();
    };
    if (win._term) win._rewire(win._term);

    // Paste. The read has no `execCommand` fallback, so on an insecure origin
    // the button is disabled (`pasteOffered`). An image becomes the same
    // `image.write` drop as a keyboard paste (ADR-0055).
    const pasteBtn = key("paste", "", "Paste", "", "bi-clipboard");
    pasteBtn.disabled = !pasteOffered(navigator.clipboard);

    key("font-down", "A−", "Smaller text");
    key("font-up", "A+", "Larger text");

    bar.addEventListener("click", (e: MouseEvent) => {
      const btn = (e.target as Element).closest<HTMLButtonElement>("button[data-key]");
      if (!btn) return;
      e.stopPropagation();
      const name = btn.dataset.key;
      // The clipboard READ goes first, before any focus move: Safari grants
      // it only to a call made synchronously inside the tap. `term.paste`
      // then rides `onData → sendInput`, so a watcher's paste is refused
      // like a keystroke. A refused or empty read is dropped silently.
      // Dormant: the terminal is off and every branch below speaks to one.
      if (!win._term) return;
      const pressed = win._term;
      const read = name === "paste" ? readClipboard() : null;
      focusWin(win);
      if (read) {
        void pasteAfterRead(win, pressed, read);
      } else if (name === "keyboard") {
        toggleKeyboard(win._term.term.textarea);
      } else if (name === "copy") {
        writeClipboard(win._term.term.getSelection(), win._term.term);
      } else if (name === "select") {
        win._term.setSelecting(!win._term.selecting);
      } else if (name === "font-up" || name === "font-down") {
        setFont(stepFont(fontSize(), name === "font-up" ? 1 : -1));
      } else {
        win._term.sendKey(name!);
      }
      // Back to the terminal, inside the gesture, so an open keyboard stays up.
      win._term.term.focus();
    });

    win.append(bar);
    applyKeyBar(win);
    return { ctrlBtn, shiftBtn, selBtn };
  }

  return {
    applyKeyBar,
    fontSize,
    setFont,
    writeClipboard,
    readClipboard,
    buildKeyBar,
  };
}
