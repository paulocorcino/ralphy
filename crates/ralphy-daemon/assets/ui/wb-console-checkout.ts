/* ---------------------------------------------------------------------------
   The console's worktree switcher: the checkout menu, the move of a console to
   another checkout, and the "new worktree" prompt.

   `createCheckoutSwitcher(deps)` returns the functions the title and the
   `WBConsole` API use (ADR-0075 D7). They read the console only through `deps`.
   `createTitle` in `wb-console-title.ts` builds it and keeps the title's text,
   the rename and the restart.
   --------------------------------------------------------------------------- */
import { WBFail } from "./wb-fail.ts";
import { WBProject } from "./wb-project.ts";
import { WBSessionRoute } from "./wb-session-route.ts";
import { WBWindowState } from "./wb-window-state.ts";
import { askWorktree } from "./wb-console-worktree.ts";
import type { ConfirmOptions } from "./wb-console-title.ts";
import type { Listing } from "./wb-project.ts";
import type { ConsoleWin, DeskWindowFields, Presentation } from "./wb-types.d.ts";

const { sessionIdOf, watchingOf } = WBWindowState;

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

// A tree of the checkout menu before the console's own state is added.
type TreeRow = Omit<CheckoutRow, "current" | "state">;

// The menu row of a worktree in a listing.
function worktreeRow(w: Listing["worktrees"][number]): TreeRow {
  return {
    name: String(w.name || ""),
    branch: String(w.branch || ""),
    dirty: w.dirty === true,
    primary: false,
  };
}

// What `checkoutMenu` is built from. `host` is where the element lands.
export type CheckoutMenuOptions = {
  anchor: HTMLElement;
  host: HTMLElement;
  rows: CheckoutRow[];
  onPick: (row: CheckoutRow) => void;
  // A Promise is allowed: the menu starts it and does not wait.
  onRemove?: (row: CheckoutRow) => void | Promise<void>;
  onCreate?: () => void;
};

// What the switcher reads from the console, and nothing else.
export type CheckoutDeps = {
  // The console's page: the store `checkouts` that holds the listing, and the
  // daemon client (`window.WBDaemon`) the worktree create goes through.
  window: Window;
  // The page the menu and the prompt build their elements in and listen on.
  document: Document;
  // The console's options: the detached popup cannot launch, so it asks for no
  // listing.
  OPTS: { canLaunch?: boolean };
  // The console windows on this page.
  wins: Set<ConsoleWin>;
  // The shell's last sessions poll: the console reassigns it, so it is read at
  // each use.
  lastSessions: () => HostedSession[];
  // The words for an agent state, the same as the console's dot.
  agentStateTitle: (state: string, detail?: string) => string;
  // The console's own confirmation dialog.
  askConfirm: (opts: ConfirmOptions) => Promise<unknown>;
  // Writes the window's fields to the desk.
  setWin: (win: ConsoleWin, fields: DeskWindowFields) => void;
  // Draws a window's title again.
  renderTitle: (win: ConsoleWin, title: HTMLElement, presentation: Presentation) => void;
};

export function createCheckoutSwitcher(deps: CheckoutDeps) {
  const {
    window,
    document,
    OPTS,
    wins,
    lastSessions,
    agentStateTitle,
    askConfirm,
    setWin,
    renderTitle,
  } = deps;

  // ---- the title's worktree segment as a switcher (#412) --------------------
  //
  // The listing per repo ref is the store `checkouts`' (wb-checkouts-store.ts),
  // which leaves out a worktree whose delete is in flight. The console holds no
  // copy: it reads the store at each use and redraws the titles of a repo when
  // the store says its listing changed (`ingestWorktrees`). Only a repo with at
  // least one worktree gets a switcher.
  function listingOf(ref: string | null | undefined) {
    return window.Alpine?.store("checkouts")?.listingOf(ref) ?? null;
  }
  function ingestWorktrees(ref: string) {
    if (!ref) return;
    for (const win of wins) {
      if (win._deskRepo === ref && win._title && win._presentation) {
        renderTitle(win, win._title, win._presentation);
      }
    }
  }
  // Asks the store to read the repo's listing: one read per ref, cached.
  function ensureListing(ref: string | null | undefined, force = false) {
    if (OPTS.canLaunch === false) return;
    void window.Alpine?.store("checkouts")?.ensureWorktreeListing(ref, force);
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
    const rows: TreeRow[] = [
      {
        name: "primary",
        branch: String(primaryBranch || ""),
        dirty: primaryDirty === true,
        primary: true,
      },
    ];
    for (const w of listing?.worktrees || []) rows.push(worktreeRow(w));
    const states =
      sessions && WBProject?.worktreeStates ? WBProject.worktreeStates(rows, sessions) : {};
    return rows.map((r) => ({
      ...r,
      current: (current ?? "primary") === r.name,
      state: states[r.name] || null,
    }));
  }
  function sessionsOfRepo(ref: string) {
    const route = WBSessionRoute;
    return (lastSessions() || []).filter(
      (s) => s && (route ? route.matchesRepo(s, ref) : s.repo === ref),
    );
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
    for (const row of rows) menu.append(checkoutItem(row, onPick, onRemove));
    if (onCreate) {
      const create = document.createElement("button");
      create.type = "button";
      create.className = "session-checkout-item create";
      create.innerHTML =
        '<i class="bi bi-folder-plus"></i><span class="session-checkout-name">New worktree…</span>';
      create.title = "Create a worktree and restart this console in it";
      create.addEventListener("click", (e) => {
        e.stopPropagation();
        closeCheckoutMenu();
        onCreate();
      });
      menu.append(create);
    }
    placeMenu(menu, anchor, host);
    host.append(menu);
    const away = (e: PointerEvent) => {
      if (!menu.contains(e.target as Node) && !anchor.contains(e.target as Node))
        closeCheckoutMenu();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeCheckoutMenu();
    };
    document.addEventListener("pointerdown", away, true);
    document.addEventListener("keydown", key, true);
    openMenu = { anchor, el: menu, away, key };
    return menu;
  }
  function checkoutGlyph(row: CheckoutRow) {
    if (row.current) return "bi bi-check2";
    return row.primary ? "bi bi-house-door" : "bi bi-folder2";
  }
  // The menu's row for one tree: its glyph, name, branch, agent state and
  // dirty flag, and a trash action when `onRemove` is given.
  function checkoutItem(
    row: CheckoutRow,
    onPick: CheckoutMenuOptions["onPick"],
    onRemove: CheckoutMenuOptions["onRemove"],
  ) {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "session-checkout-item" + (row.current ? " current" : "");
    const glyph = document.createElement("i");
    glyph.className = checkoutGlyph(row);
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
      item.append(removeAction(row, onRemove));
    }
    item.addEventListener("click", (e) => {
      e.stopPropagation();
      closeCheckoutMenu();
      if (!row.current) onPick(row);
    });
    return item;
  }
  // The trash action of a worktree row. A span, not a button: a button inside
  // a button is not HTML, and the browser would hoist it out of the row.
  function removeAction(row: CheckoutRow, onRemove: NonNullable<CheckoutMenuOptions["onRemove"]>) {
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
      void onRemove(row);
    });
    return trash;
  }
  // Puts the menu under its anchor: fixed over the page for the shell's chip
  // (`host` is the body), else relative to the console window `host`.
  function placeMenu(menu: HTMLElement, anchor: HTMLElement, host: HTMLElement) {
    const r = anchor.getBoundingClientRect();
    if (host === document.body) {
      menu.style.position = "fixed";
      menu.style.left = `${r.left}px`;
      menu.style.top = `${r.bottom + 2}px`;
      return;
    }
    const h = host.getBoundingClientRect();
    menu.style.left = `${Math.max(0, r.left - h.left)}px`;
    menu.style.top = `${r.bottom - h.top + 2}px`;
  }
  function openCheckoutMenu(win: ConsoleWin, anchor: HTMLElement) {
    const ref = win._deskRepo;
    checkoutMenu({
      anchor,
      host: win,
      rows: checkoutMenuRows(listingOf(ref), win._deskCheckout ?? null, sessionsOfRepo(ref)),
      onPick: (row) => {
        void switchCheckout(win, row.primary ? null : row.name);
      },
      onCreate: () => {
        void createWorktreeFor(win);
      },
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
      message:
        `Restarts the ${win._deskAgent} session in ${where}. ` +
        "You lose the text in this console.",
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
      const relaunchIn = win._relaunchIn;
      if (typeof relaunchIn !== "function") return;
      relaunchIn(checkout);
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

  // `+ new worktree…` from a console's switcher: ask, `worktree.add`, tell the
  // shell, and move THIS console into it (the prompt already said it restarts).
  // Base list is `branch.list`, one read per prompt; unreadable → free text.
  async function createWorktreeFor(win: ConsoleWin) {
    const repo = win._deskRepo;
    if (!repo || repo === "~" || typeof win._relaunchIn !== "function") return;
    const daemon = window.WBDaemon;
    if (typeof daemon?.observe !== "function") return;
    const { branches, base: first } = await readBranches(daemon, repo);
    let base = first;
    let error = "";
    let name = "";
    for (;;) {
      const ask = await askWorktree(document, {
        base,
        branches,
        listing: listingOf(repo),
        error,
        name,
      });
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
      const refused = addRefusal(reply);
      if (refused) {
        error = refused;
        continue;
      }
      announceWorktree(repo, name, reply);
      moveTo(win, name);
      return;
    }
  }
  // The words of a `worktree.add` refusal for the prompt, or "" when the
  // worktree was added.
  function addRefusal(reply: ReplyOf<"worktree.add">) {
    if (reply && reply.status === "ok") return "";
    return WBFail.failed(reply, "Could not create the worktree: the daemon gave no reason.");
  }
  // The branches the prompt offers and the one it starts from; unreadable
  // gives no list and no base.
  async function readBranches(daemon: NonNullable<Window["WBDaemon"]>, repo: string) {
    let branches: string[] = [];
    let base = "";
    try {
      const reply = await daemon.observe("branch.list", { repo });
      const data = (reply && reply.status === "ok" && reply.branches) || {};
      if (Array.isArray(data.branches)) branches = data.branches;
      if (data.current && data.current !== "HEAD") base = data.current;
    } catch {}
    return { branches, base };
  }
  // The shell is told the worktree exists; it reads the listing again.
  function announceWorktree(repo: string, name: string, reply: { message?: unknown }) {
    WB.emit("worktree-created", {
      project: repo,
      name,
      message: typeof reply.message === "string" ? reply.message : "",
    });
  }

  return {
    ingestWorktrees,
    ensureListing,
    checkoutMenuRows,
    closeCheckoutMenu,
    checkoutMenu,
    openCheckoutMenu,
    endLiveThen,
  };
}
