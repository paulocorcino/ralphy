/* ---------------------------------------------------------------------------
   The console's title: the console name and its rename, the title's text,
   and the restart. The worktree switcher on the title (the checkout menu, the
   move to another checkout, and the "new worktree" prompt) is
   `wb-console-checkout.ts`.

   `createTitle(deps)` returns the functions the console, its chrome and the
   `WBConsole` API use (ADR-0075 D7). They read the console only through
   `deps`, and `TitleDeps` lists every read, so `tsc` refuses a read outside
   it. `wb-console.ts` creates one per console, before its chrome, and keeps
   the desk, the sessions poll and the project names.
   --------------------------------------------------------------------------- */
import { WBConsoleName } from "./wb-console-name.ts";
import { createCheckoutSwitcher } from "./wb-console-checkout.ts";
import { agentStateTitle } from "./wb-console-session.ts";
import { paintTabShadow } from "./wb-console-place.ts";
import type { ConsoleWin, DeskRecord, DeskWindowFields, Presentation } from "./wb-types.d.ts";

// What the console's confirmation dialog takes.
export type ConfirmOptions = {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
  notice?: boolean;
};

// What the title reads from the console, and nothing else.
export type TitleDeps = {
  // The console's page: the daemon client (`window.WBDaemon`) the listing
  // read and the worktree create go through.
  window: Window;
  // The page the title builds its elements in and listens on.
  document: Document;
  // The console's options: the detached popup cannot launch, so it has no
  // switcher, no listing read and no rename.
  OPTS: { canLaunch?: boolean };
  // The console windows on this page.
  wins: Set<ConsoleWin>;
  // Every console window of the page.
  pageWindows: () => ConsoleWin[];
  // The desk mirror and the shell's last sessions poll: the console
  // reassigns both, so they are read at each use.
  desk: () => DeskRecord[];
  lastSessions: () => HostedSession[];
  // The console's own confirmation dialog.
  askConfirm: (opts: ConfirmOptions) => Promise<unknown>;
  // A repo ref's project name, and its tooltip text.
  projectNameOf: (ref: string | null | undefined) => string;
  projectTitleOf: (ref: string | null | undefined) => string;
  // Writes the window's fields to the desk.
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
};

export function createTitle(deps: TitleDeps) {
  const {
    window,
    document,
    OPTS,
    wins,
    pageWindows,
    desk,
    lastSessions,
    askConfirm,
    projectNameOf,
    projectTitleOf,
    setWin,
  } = deps;

  // The console name's prefix is the PROJECT NAME's last segment: a peer ref's
  // routing head never shows, a remoteless repo is named by its folder and not
  // its `path-<hash>` key, and the same repo on two environments shares one
  // count (ADR-0066 §2).
  function consolePrefix(repo: string | null | undefined) {
    return WBConsoleName.prefixOf(projectNameOf(repo));
  }
  // Every console name in use: the desk mirror's and the stage's windows',
  // except `exceptId` (the console being renamed).
  function takenNames(exceptId: string | null) {
    const names = desk().filter((r) => r.id !== exceptId).map((r) => r.consoleName);
    for (const w of pageWindows()) {
      if (w._deskId !== exceptId) names.push(w._deskConsoleName);
    }
    return names.filter(Boolean);
  }

  // The title is built by `renderTitle` from the window's console name and
  // label (ADR-0066 §4). The environment left the title for the tooltip
  // (ADR-0066 §5), after the full ref.
  function sessionPresentation(
    _label: string | null | undefined,
    repo: string | null | undefined,
    prior: { daemonId?: string | null; environment?: string | null } | null | undefined,
    owner: { daemon_id?: string; environment?: string; name?: string | null; checkout?: string | null } | null | undefined,
  ) {
    const { daemonId, environment } = sessionPlace(prior, owner);
    // The vendor's own session name (`--name`; Claude only). On the TOOLTIP, not
    // the title: a fourth segment would outrun the titlebar. NO desk fallback:
    // the name dies with the child and the daemon re-announces it on every
    // reattach, so a restored window with no socket yet correctly shows none.
    const name = owner?.name ?? null;
    // The worktree the console lives in (ADR-0063 §3), right after the label.
    // From the `session-open` payload ONLY — no desk fallback, like `name` — so
    // changing the picker's selection can never retitle a live console.
    const checkout = owner?.checkout ?? null;
    return {
      daemonId,
      environment,
      name,
      checkout,
      tooltip: WBConsoleName.tooltipLines(projectTitleOf(repo), environment, name).join("\n"),
    };
  }

  const {
    ingestWorktrees,
    ensureListing,
    checkoutMenuRows,
    closeCheckoutMenu,
    checkoutMenu,
    openCheckoutMenu,
    endLiveThen,
  } = createCheckoutSwitcher({
    window,
    document,
    OPTS,
    wins,
    lastSessions,
    agentStateTitle,
    askConfirm,
    setWin,
    renderTitle,
  });


  // The title: `<console name> (<label>)`, then ` · <checkout>`, then
  // ` · <repo slug>` (ADR-0066 §4).
  // On an agentic console the checkout segment is ALWAYS a button (`primary` on
  // the primary tree): it is where the first worktree is born via `+ new
  // worktree…`, so it cannot wait for one to exist (ADR-0063, amendment
  // 2026-09-16 b). Never on a plain shell (stays on the primary, #408), a
  // placeholder (no `_relaunchIn`) or the detached popup (`canLaunch === false`).
  function renderTitle(win: ConsoleWin, title: HTMLElement, presentation: Presentation) {
    win._presentation = presentation;
    // A rename in progress keeps its input; `endEdit` draws with the latest.
    if (title.querySelector(".session-name-input")) return;
    paintTabShadow(win);
    const switchable =
      win._deskKind !== "console" &&
      typeof win._relaunchIn === "function" &&
      OPTS.canLaunch !== false;
    title.textContent = "";
    const icon = document.createElement("i");
    icon.className = "bi bi-terminal";
    title.append(icon, " ");
    // SPANS, not bare text nodes: they are what ellipsise when the bar is
    // narrow, the repo first (06-consoles.css `.session-repo`); a text node
    // inside an inline-flex box wraps instead.
    const parts = WBConsoleName.labelParts(win._deskConsoleName || "", win._deskAgent);
    const nameSpan = document.createElement("span");
    nameSpan.className = "session-name";
    nameSpan.textContent = parts.name;
    const labelSpan = document.createElement("span");
    labelSpan.className = "session-label";
    labelSpan.textContent = parts.tag;
    title.append(nameSpan, " ", labelSpan);
    wireRename(win, nameSpan);
    if (switchable) appendSwitches(win, title, presentation);
    // The project name closes the title and is the first text cut; a console
    // with no repo has none, its default name already says `home`.
    const repo = win._deskRepo && win._deskRepo !== "~" ? projectNameOf(win._deskRepo) : "";
    if (repo) {
      const repoSpan = document.createElement("span");
      repoSpan.className = "session-repo";
      // The dot is inside the span, so a repo cut to nothing leaves no dot.
      repoSpan.textContent = `· ${repo}`;
      title.append(" ", repoSpan);
    }
  }

  // The buttons an agent console keeps in its title: the checkout segment,
  // then the done mark's bell.
  function appendSwitches(win: ConsoleWin, title: HTMLElement, presentation: Presentation) {
    appendCheckout(win, title, presentation);
    appendDoneMark(win, title);
  }

  // Whether the desk says this console has "Tell me when it is done" on. The
  // daemon clears it when the request opens, so it is read at each use.
  function doneMarkOn(win: ConsoleWin) {
    return !!desk().find((r) => r.id === win._deskId)?.doneMark;
  }

  // "Tell me when it is done" (docs/adr/0085 D11): a bell that asks for one
  // request at the next turn this console finishes. A click writes the mark
  // to the desk; the daemon clears it when the request opens. The bell is
  // drawn from the desk again when the pointer comes to it, so a mark the
  // daemon cleared is not shown as on.
  function appendDoneMark(win: ConsoleWin, title: HTMLElement) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "session-checkout session-done-mark";
    const icon = document.createElement("i");
    const paint = () => {
      const on = doneMarkOn(win);
      icon.className = on ? "bi bi-bell-fill" : "bi bi-bell";
      btn.setAttribute("aria-pressed", String(on));
      btn.title = on
        ? "Tell me when it is done: on. Click to turn it off."
        : "Tell me when it is done";
    };
    paint();
    btn.append(icon);
    btn.addEventListener("pointerdown", (e) => e.stopPropagation());
    btn.addEventListener("pointerenter", paint);
    btn.addEventListener("focus", paint);
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      setWin(win, { doneMark: !doneMarkOn(win) });
      paint();
    });
    // No text node before the bell: it has no text, so a space would leave
    // two spaces in the title's text (`wt-a  · repo`). The gap is CSS.
    title.append(btn);
  }

  // The checkout segment of the title: ` · <checkout> ▾`.
  function appendCheckout(win: ConsoleWin, title: HTMLElement, presentation: Presentation) {
    const sep = document.createElement("span");
    sep.className = "session-title-sep";
    sep.textContent = "·";
    title.append(" ", sep, " ");
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "session-checkout";
    btn.title = "Switch worktree";
    // Before `session-open` the announcement has not come; the record says
    // where the console was asked to run (#411), never the picker.
    btn.textContent = presentation.checkout ?? win._deskCheckout ?? "primary";
    const caret = document.createElement("i");
    caret.className = "bi bi-chevron-down";
    btn.append(caret);
    btn.addEventListener("pointerdown", (e) => e.stopPropagation());
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openCheckoutMenu(win, btn);
    });
    title.append(btn);
  }

  // Rename a console from its title (ADR-0066 §3): the fence rename's rules.
  // At rest the name is plain text; a mouse double-click, or a finger held on
  // the name (`wireTitleTouch`), swaps in an input, and `endEdit` — the ONE
  // place an edit ends — draws the title again, which puts the text back.
  // Enter commits; Escape, a press anywhere else and a focus loss cancel. No
  // lock check: a locked or fenced console can be renamed. The detached popup
  // cannot: its sink stores nothing.
  function canRename() {
    return OPTS.canLaunch !== false;
  }
  function wireRename(win: ConsoleWin, span: HTMLElement) {
    if (!canRename()) return;
    span.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      // On touch a double tap maximizes, even on the name.
      if (win._lastPointerType !== "mouse") return;
      startRename(win, span);
    });
  }
  function startRename(win: ConsoleWin, span: HTMLElement) {
    if (!span.isConnected) return;
    const input = document.createElement("input");
    input.className = "session-name-input";
    input.setAttribute("aria-label", "Console name");
    input.maxLength = WBConsoleName.NAME_MAX;
    input.value = win._deskConsoleName || "";
    input.style.width = `${Math.max(span.offsetWidth + 16, 96)}px`;
    let editing = true;
    // Capture phase, before the plane's pan handler swallows the press: the
    // pan calls `preventDefault()` on mousedown, so focus does not move.
    // Also ends an edit whose window left the page: not every browser fires
    // `blur` on a removed input.
    const stopOutside = (ev: PointerEvent) => {
      if (ev.target !== input || !win.isConnected) endEdit(false);
    };
    const endEdit = (commit: boolean) => {
      if (!editing) return;
      editing = false; // first: removing the input fires `blur`, which re-enters
      document.removeEventListener("pointerdown", stopOutside, true);
      if (commit) {
        win._deskConsoleName = WBConsoleName.renameValue(
          input.value,
          consolePrefix(win._deskRepo),
          takenNames(win._deskId),
        );
        setWin(win, { consoleName: win._deskConsoleName });
      } else {
        // A name another page gave while this edit was open was skipped by
        // `converge`; take it now.
        const stored = desk().find((r) => r.id === win._deskId)?.consoleName;
        if (stored) win._deskConsoleName = stored;
      }
      input.remove();
      if (win._title && win._presentation) renderTitle(win, win._title, win._presentation);
    };
    input.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    input.addEventListener("dblclick", (ev) => ev.stopPropagation());
    input.addEventListener("keydown", (ev) => {
      // Held here so an Escape meant for this edit never reaches the plane.
      ev.stopPropagation();
      // An Enter that confirms an IME candidate is not a commit.
      if (ev.isComposing) return;
      if (ev.key === "Enter" || ev.key === "Escape") endEdit(ev.key === "Enter");
    });
    input.addEventListener("blur", () => endEdit(false));
    span.replaceWith(input);
    input.focus();
    input.select();
    document.addEventListener("pointerdown", stopOutside, true);
  }

  // The titlebar's restart: offered on a live session too, so it always asks
  // first — one click beside maximize must not tree-kill a working agent.
  async function restartWin(win: ConsoleWin) {
    if (typeof win._relaunchIn !== "function") return;
    const ended = win.classList.contains("ended");
    const ok = await askConfirm({
      title: "Restart session?",
      message: ended
        ? `Starts a fresh ${win._deskAgent || "console"} session in this window. You lose the text in this console.`
        : `Ends the running ${win._deskAgent || "console"} session and starts a fresh one. You lose the text in this console.`,
      confirmLabel: "Restart",
      danger: !ended,
    });
    if (!ok) return;
    endLiveThen(win, () => win._relaunchIn!(undefined));
  }


  return {
    consolePrefix,
    takenNames,
    sessionPresentation,
    ingestWorktrees,
    ensureListing,
    checkoutMenuRows,
    renderTitle,
    canRename,
    startRename,
    closeCheckoutMenu,
    checkoutMenu,
    restartWin,
  };
}

// The daemon and the environment a session runs on: the `session-open`
// payload's, else the record's, else none.
function sessionPlace(
  prior: { daemonId?: string | null; environment?: string | null } | null | undefined,
  owner: { daemon_id?: string; environment?: string } | null | undefined,
) {
  return {
    daemonId: owner?.daemon_id ?? prior?.daemonId ?? null,
    environment: owner?.environment ?? prior?.environment ?? null,
  };
}
