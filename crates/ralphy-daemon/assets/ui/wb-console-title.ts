/* ---------------------------------------------------------------------------
   The console's title: the console name and its rename, the title's text,
   and the worktree switcher on the title (the checkout menu, the move to
   another checkout, the restart, and the "new worktree" prompt).

   `createTitle(deps)` returns the functions the console, its chrome and the
   `WBConsole` API use (ADR-0075 D7). They read the console only through
   `deps`, and `TitleDeps` lists every read, so `tsc` refuses a read outside
   it. `wb-console.ts` creates one per console, before its chrome, and keeps
   the desk, the sessions poll and the project names.
   --------------------------------------------------------------------------- */
import { WBConsoleName } from "./wb-console-name.ts";
import { WBFail } from "./wb-fail.ts";
import { WBProject } from "./wb-project.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { WBWindowState } from "./wb-window-state.ts";
import type { Listing } from "./wb-project.ts";
import type { ConsoleWin, DeskRecord, DeskWindowFields, Presentation } from "./wb-types.d.ts";

const { sessionIdOf, watchingOf } = WBWindowState;

// What the console's confirmation dialog takes.
export type ConfirmOptions = {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
  notice?: boolean;
};

// A row of the checkout menu: a tree, its dirty flag, the agent state in it,
// and whether it is the one the console is in.
export type CheckoutRow = {
  name: string;
  branch: string;
  dirty: boolean;
  primary: boolean;
  current: boolean;
  state: string | null;
};

// What `checkoutMenu` is built from. `host` is where the element lands.
export type CheckoutMenuOptions = {
  anchor: HTMLElement;
  host: HTMLElement;
  rows: CheckoutRow[];
  onPick: (row: CheckoutRow) => void;
  onRemove?: (row: CheckoutRow) => void;
  onCreate?: () => void;
};

// What the new worktree prompt is opened with, and what it answers.
type WorktreeAsk = {
  base: string;
  branches: string[];
  listing: Listing | null | undefined;
  error?: string;
  name?: string;
};
type WorktreeAnswer = { name: string; base: string } | null;

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
  // The plane; null before the page has it.
  stage: () => HTMLElement | null;
  // The desk mirror and the shell's last sessions poll: the console
  // reassigns both, so they are read at each use.
  desk: () => DeskRecord[];
  lastSessions: () => HostedSession[];
  // The words for an agent state, the same as the console's dot.
  agentStateTitle: (state: string, detail?: string) => string;
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
    stage,
    desk,
    lastSessions,
    agentStateTitle,
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
    const st = typeof document?.getElementById === "function" ? stage() : null;
    for (const w of st ? st.querySelectorAll<ConsoleWin>(".session-window") : []) {
      if (w._deskId !== exceptId) names.push(w._deskConsoleName);
    }
    return names.filter(Boolean);
  }

  // The title is built by `renderTitle` from the window's console name and
  // label (ADR-0066 §4). The environment left the title for the tooltip
  // (ADR-0066 §5), after the full ref.
  function sessionPresentation(
    label: string | null | undefined,
    repo: string | null | undefined,
    prior: { daemonId?: string | null; environment?: string | null } | null | undefined,
    owner: { daemon_id?: string; environment?: string; name?: string | null; checkout?: string | null } | null | undefined,
  ) {
    const daemonId = owner?.daemon_id ?? prior?.daemonId ?? null;
    const environment = owner?.environment ?? prior?.environment ?? null;
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

  // ---- the title's worktree segment as a switcher (#412) --------------------
  //
  // Listing per repo ref. Only a repo with at least one worktree gets a
  // switcher. Fed by the shell (`ingestWorktrees`, from every `worktree.list` it
  // reads for the picker) and, for a repo the picker never opened, by ONE read
  // of our own per ref at the first agent window — `worktree.list` is a git
  // spawn, so never per render and never periodic.
  const worktreeListings: Record<string, Listing | null> = {};
  const listingReads = new Map<string, Promise<void>>();
  function ingestWorktrees(ref: string, listing: Listing | null | undefined) {
    if (!ref) return;
    worktreeListings[ref] = listing || null;
    for (const win of wins) {
      if (win._deskRepo === ref && win._title && win._presentation) {
        renderTitle(win, win._title, win._presentation);
      }
    }
  }
  function ensureListing(ref: string | null | undefined, force = false) {
    if (!ref || ref === "~" || (ref in worktreeListings && !force) || listingReads.has(ref)) return;
    const daemon = window.WBDaemon;
    if (typeof daemon?.observe !== "function" || OPTS.canLaunch === false) return;
    const read = daemon
      .observe("worktree.list", { repo: ref })
      .then((reply) => {
        ingestWorktrees(ref, reply && reply.status === "ok" ? reply.checkouts || null : null);
      })
      .catch(() => ingestWorktrees(ref, null))
      .finally(() => listingReads.delete(ref));
    listingReads.set(ref, read);
  }
  // The switcher's rows: `primary` first, then the worktrees in listing order,
  // each with its dirty flag; the current one marked. With `sessions` (rows with
  // `checkout` and `agent_state`) each row also carries the agent's state in
  // that tree (ADR-0059 §5) via `WBProject.worktreeStates`. `primaryBranch`/
  // `primaryDirty` when the caller knows them (the shell does).
  function checkoutMenuRows(
    listing: Listing | null | undefined,
    current: string | null | undefined,
    sessions: HostedSession[] | null | undefined,
    primaryBranch = "",
    primaryDirty = false,
  ): CheckoutRow[] {
    const rows: { name: string; branch: string; dirty: boolean; primary: boolean }[] = [
      { name: "primary", branch: String(primaryBranch || ""), dirty: primaryDirty === true, primary: true },
    ];
    for (const w of listing?.worktrees || []) {
      rows.push({
        name: String(w.name || ""),
        branch: String(w.branch || ""),
        dirty: w.dirty === true,
        primary: false,
      });
    }
    const states = sessions && WBProject?.worktreeStates ? WBProject.worktreeStates(rows, sessions) : {};
    return rows.map((r) => ({ ...r, current: (current ?? "primary") === r.name, state: states[r.name] || null }));
  }
  function sessionsOfRepo(ref: string) {
    const route = WBSessionRoute;
    return (lastSessions() || []).filter((s) => s && (route ? route.matchesRepo(s, ref) : s.repo === ref));
  }

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
    if (switchable) appendCheckout(win, title, presentation);
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

  // The checkout menu — ONE component, under the console's title segment and
  // under the Files bar's chip. `host` is where the element lands (a console
  // window, or the document for the shell) and what it is positioned against;
  // `onPick(row)` for a non-current row; `onRemove(row)` adds a trash action per
  // worktree row; `onCreate()` adds `+ new worktree…`. One menu at a time;
  // closes on a pick, a click elsewhere, or Escape.
  let openMenu: {
    anchor: HTMLElement;
    el: HTMLElement;
    away: (e: PointerEvent) => void;
    key: (e: KeyboardEvent) => void;
  } | null = null;
  function closeCheckoutMenu() {
    if (!openMenu) return;
    openMenu.el.remove();
    document.removeEventListener("pointerdown", openMenu.away, true);
    document.removeEventListener("keydown", openMenu.key, true);
    openMenu = null;
  }
  function checkoutMenu({ anchor, host, rows, onPick, onRemove, onCreate }: CheckoutMenuOptions) {
    if (openMenu?.anchor === anchor) return closeCheckoutMenu();
    closeCheckoutMenu();
    const menu = document.createElement("div");
    menu.className = "session-checkout-menu";
    menu.addEventListener("pointerdown", (e) => e.stopPropagation());
    for (const row of rows) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "session-checkout-item" + (row.current ? " current" : "");
      const glyph = document.createElement("i");
      glyph.className = row.current ? "bi bi-check2" : row.primary ? "bi bi-house-door" : "bi bi-folder2";
      const name = document.createElement("span");
      name.className = "session-checkout-name";
      name.textContent = row.name;
      item.append(glyph, name);
      if (row.branch) {
        const branch = document.createElement("span");
        branch.className = "session-checkout-branch";
        branch.textContent = row.branch;
        item.append(branch);
      }
      // The agent in this tree (ADR-0059): the same words as the console's dot.
      if (row.state) {
        const state = document.createElement("span");
        state.className = `session-checkout-state ${row.state}`;
        state.title = agentStateTitle(row.state);
        item.append(state);
      }
      if (row.dirty) {
        const dot = document.createElement("span");
        dot.className = "session-checkout-dirty";
        dot.title = "Uncommitted changes";
        item.append(dot);
      }
      if (onRemove && !row.primary) {
        // A span, not a button: a button inside a button is not HTML, and the
        // browser would hoist it out of the row.
        const trash = document.createElement("span");
        trash.setAttribute("role", "button");
        trash.tabIndex = 0;
        trash.className = "session-checkout-remove";
        trash.title = "Delete this worktree";
        trash.innerHTML = '<i class="bi bi-trash3"></i>';
        // The trash must not also PICK the row it sits on.
        trash.addEventListener("click", (e) => {
          e.stopPropagation();
          closeCheckoutMenu();
          onRemove(row);
        });
        item.append(trash);
      }
      item.addEventListener("click", (e) => {
        e.stopPropagation();
        closeCheckoutMenu();
        if (!row.current) onPick(row);
      });
      menu.append(item);
    }
    if (onCreate) {
      const create = document.createElement("button");
      create.type = "button";
      create.className = "session-checkout-item create";
      create.innerHTML = '<i class="bi bi-folder-plus"></i><span class="session-checkout-name">New worktree…</span>';
      create.title = "Create a worktree and restart this console in it";
      create.addEventListener("click", (e) => {
        e.stopPropagation();
        closeCheckoutMenu();
        onCreate();
      });
      menu.append(create);
    }
    const r = anchor.getBoundingClientRect();
    if (host === document.body) {
      // The shell's chip: the menu floats over the page, at the anchor.
      menu.style.position = "fixed";
      menu.style.left = `${r.left}px`;
      menu.style.top = `${r.bottom + 2}px`;
    } else {
      const h = host.getBoundingClientRect();
      menu.style.left = `${Math.max(0, r.left - h.left)}px`;
      menu.style.top = `${r.bottom - h.top + 2}px`;
    }
    host.append(menu);
    const away = (e: PointerEvent) => {
      if (!menu.contains(e.target as Node) && !anchor.contains(e.target as Node)) closeCheckoutMenu();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeCheckoutMenu();
    };
    document.addEventListener("pointerdown", away, true);
    document.addEventListener("keydown", key, true);
    openMenu = { anchor, el: menu, away, key };
    return menu;
  }
  function openCheckoutMenu(win: ConsoleWin, anchor: HTMLElement) {
    const ref = win._deskRepo;
    checkoutMenu({
      anchor,
      host: win,
      rows: checkoutMenuRows(worktreeListings[ref], win._deskCheckout ?? null, sessionsOfRepo(ref)),
      onPick: (row) => switchCheckout(win, row.primary ? null : row.name),
      onCreate: () => createWorktreeFor(win),
    });
  }

  // Move a console to another checkout (#412): confirm (the session restarts
  // and its scrollback goes), then `moveTo`. The picker's per-repo selection is
  // never touched: that is what Files shows; this is where THIS console lives.
  async function switchCheckout(win: ConsoleWin, checkout: string | null) {
    if (typeof win._relaunchIn !== "function") return;
    const where = checkout ? `worktree ${checkout}` : "the primary tree";
    const ok = await askConfirm({
      title: `Restart in ${checkout ?? "primary"}?`,
      message: `Restarts the ${win._deskAgent} session in ${where}. You lose the text in this console.`,
      confirmLabel: "Restart",
    });
    if (!ok) return;
    moveTo(win, checkout);
  }
  // The record is written with the choice BEFORE anything is requested, so a
  // daemon that dies mid-launch still leaves the intent behind. A LIVE session
  // is ended on the daemon first (`/api/sessions/close`): `relaunchIn` was
  // built for an ended child, and moving a running one left the old session
  // alive with no window (measured 2026-09-16). A watcher holds no baton and
  // must not kill the child another operator drives; it just relaunches.
  function moveTo(win: ConsoleWin, checkout: string | null) {
    const from = win._deskCheckout ?? null;
    win._deskCheckout = checkout;
    setWin(win, { checkout: checkout ?? null });
    endLiveThen(win, () => {
      // Both callers checked that `_relaunchIn` is a function.
      win._relaunchIn!(checkout);
      WB.emit("console-switch-checkout", { repo: win._deskRepo, from, to: checkout });
    });
  }
  // End this window's session on the daemon if it is still running, then `go`.
  // A DORMANT console still holds its session and must still close it.
  function endLiveThen(win: ConsoleWin, go: () => void) {
    const id = sessionIdOf(win);
    const live = id != null && !win.classList.contains("ended") && !watchingOf(win);
    if (live && WBSessionRoute) {
      fetch(WBSessionRoute.closeUrl(id, win._deskRepo), { method: "POST" }).then(go, go);
    } else {
      go();
    }
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

  // The "new worktree" prompt: a name (worktree AND branch, ADR-0063 §2) and
  // the base branch. The name gate is `WBProject.worktreeCreateRow`; a refusal
  // the daemon DID send (`error`) re-opens with the message under the field.
  // Resolves `{name, base}` or `null` on cancel.
  function askWorktree({ base, branches, listing, error = "", name = "" }: WorktreeAsk): Promise<WorktreeAnswer> {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-confirm";
    const modal = document.createElement("div");
    modal.className = "modal confirm-modal wb-worktree";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", "New worktree");
    const head = document.createElement("div");
    head.className = "modal-head";
    head.innerHTML = '<i class="bi bi-folder-plus"></i>';
    const heading = document.createElement("span");
    heading.className = "modal-title";
    heading.textContent = "New worktree";
    head.append(heading);
    const form = document.createElement("div");
    form.className = "wb-worktree-form";
    const nameLabel = document.createElement("label");
    nameLabel.textContent = "Name";
    const nameInput = document.createElement("input");
    nameInput.className = "prompt-input";
    nameInput.placeholder = "Worktree and branch name";
    nameInput.value = name;
    const baseLabel = document.createElement("label");
    baseLabel.textContent = "From";
    const baseInput = document.createElement(branches?.length ? "select" : "input");
    baseInput.className = "prompt-input";
    if (branches?.length) {
      for (const b of branches) {
        const opt = document.createElement("option");
        opt.value = b;
        opt.textContent = b;
        baseInput.append(opt);
      }
      baseInput.value = branches.includes(base) ? base : branches[0];
    } else {
      baseInput.value = base || "";
      (baseInput as HTMLInputElement).placeholder = "Branch to start from";
    }
    const note = document.createElement("p");
    note.className = "wb-worktree-note";
    note.textContent = `${WBProject?.CARRY_OVER_NOTE || ""} The console restarts in the new worktree. You lose the text in this console.`;
    const err = document.createElement("p");
    err.className = "prompt-error";
    err.textContent = error;
    err.hidden = !error;
    form.append(nameLabel, nameInput, baseLabel, baseInput, note, err);
    const foot = document.createElement("div");
    foot.className = "modal-foot";
    const cancel = document.createElement("button");
    cancel.className = "btn";
    cancel.type = "button";
    cancel.textContent = "Cancel";
    const go = document.createElement("button");
    go.className = "btn accent";
    go.type = "button";
    go.textContent = "Create & restart";
    foot.append(cancel, go);
    modal.append(head, form, foot);
    scrim.append(modal);
    document.body.append(scrim);
    nameInput.focus();
    nameInput.select();

    return new Promise((resolve) => {
      let settled = false;
      const done = (value: WorktreeAnswer) => {
        if (settled) return;
        settled = true;
        document.removeEventListener("keydown", onKey, true);
        scrim.remove();
        resolve(value);
      };
      const problem = () =>
        WBProject?.worktreeNameProblem?.(listing || { worktrees: [] }, nameInput.value) || "";
      const submit = () => {
        const row = WBProject?.worktreeCreateRow?.(listing || { worktrees: [] }, baseInput.value, nameInput.value);
        if (!row) {
          err.textContent = problem() || "Choose a branch to start from.";
          err.hidden = false;
          nameInput.focus();
          return;
        }
        done({ name: row.name, base: row.base });
      };
      // The mask runs on every edit, typed or pasted: a refused character
      // never shows, and there is no message. The caret stays after the
      // last kept character. The create stays disabled, without a message,
      // while the name is still one the daemon would refuse.
      const mask = WBProject?.maskWorktreeName || ((s: string) => s);
      nameInput.addEventListener("input", () => {
        const raw = nameInput.value;
        const masked = mask(raw);
        if (masked !== raw) {
          const caret = mask(raw.slice(0, nameInput.selectionStart ?? raw.length)).length;
          nameInput.value = masked;
          nameInput.setSelectionRange(caret, caret);
        }
        err.textContent = "";
        err.hidden = true;
        go.disabled = !!problem();
      });
      go.disabled = !!problem();
      const onKey = (e: KeyboardEvent) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          done(null);
        } else if (e.key === "Enter" && modal.contains(document.activeElement)) {
          e.stopPropagation();
          if (document.activeElement === cancel) done(null);
          else submit();
        }
      };
      document.addEventListener("keydown", onKey, true);
      cancel.addEventListener("click", () => done(null));
      go.addEventListener("click", submit);
    });
  }

  // `+ new worktree…` from a console's switcher: ask, `worktree.add`, tell the
  // shell, and move THIS console into it (the prompt already said it restarts).
  // Base list is `branch.list`, one read per prompt; unreadable → free text.
  async function createWorktreeFor(win: ConsoleWin) {
    const repo = win._deskRepo;
    if (!repo || repo === "~" || typeof win._relaunchIn !== "function") return;
    const daemon = window.WBDaemon;
    if (typeof daemon?.observe !== "function") return;
    let branches: string[] = [];
    let base = "";
    try {
      const reply = await daemon.observe("branch.list", { repo });
      const data = (reply && reply.status === "ok" && reply.branches) || {};
      if (Array.isArray(data.branches)) branches = data.branches;
      if (data.current && data.current !== "HEAD") base = data.current;
    } catch {}
    let error = "";
    let name = "";
    for (;;) {
      const ask = await askWorktree({ base, branches, listing: worktreeListings[repo], error, name });
      if (!ask) return;
      name = ask.name;
      base = ask.base;
      let reply;
      try {
        reply = await daemon.observe("worktree.add", { repo, name, base });
      } catch {
        error = "Could not reach the daemon. Check whether the worktree was created.";
        continue;
      }
      if (!reply || reply.status !== "ok") {
        error = WBFail.failed(reply, "Could not create the worktree: the daemon gave no reason.");
        continue;
      }
      ensureListing(repo, true);
      WB.emit("worktree-created", { project: repo, name, message: typeof reply.message === "string" ? reply.message : "" });
      moveTo(win, name);
      return;
    }
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
