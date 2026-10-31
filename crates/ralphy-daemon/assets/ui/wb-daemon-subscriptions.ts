/* ---------------------------------------------------------------------------
   ralphy workbench shell — the subscriptions on the page socket

   The tree, runs and changes subscriptions of a project. Each holds a
   channel of its own on the page's one socket (ADR-0082), which
   `wb-daemon.ts` owns: it gives this module the socket's `join` and
   `resume`, and its frame codec.
--------------------------------------------------------------------------- */
import type { Channel, ChannelKind, CloseReason, PageChannels } from "./wb-channels.ts";

/** One user of the page socket: presence, or a subscription with its channel. */
export type SocketUser = {
  /** Each open of the socket, and the join when it is open already.
   * `reopened` is false on this user's first open only. */
  onOpen: (channels: PageChannels, reopened: boolean) => void;
  /** A bare frame of channel 0: the heartbeat and the pushes. */
  onBare?: (bytes: Uint8Array) => void;
  /** The page replaces the socket: a last frame sent here goes before the
   * close. */
  onResume?: () => void;
};

/** What a subscription on a channel of the page socket hears. */
type ChannelHandlers = {
  /** Each open of the channel; `send` sends one frame on it. */
  onOpen: (send: (frame: Uint8Array) => boolean, reopened: boolean) => void;
  /** Each command frame the daemon sends on the channel. */
  onPush: (frame: PushFrame) => void;
};

/** The page socket, as its subscriptions use it. */
export type PageSocket = {
  /** Add a user; it returns the user's leave. */
  join: (user: SocketUser) => () => void;
  resume: (stale: boolean) => boolean;
};

/** What `wb-daemon.ts` gives the subscriptions. */
export type SubscriptionDeps = {
  page: PageSocket;
  encodeCommand: (command: { id: number; verb: string; payload: CommandPayload }) => Uint8Array;
  withCheckout: (payload: CommandPayload, checkout: string | null | undefined) => CommandPayload;
  /** A `[0x02][JSON]` command frame, or `null` for anything else. */
  commandFrame: (bytes: Uint8Array) => PushFrame | null;
  /** The wait before a channel the daemon ended is opened again: the
   * socket's own reconnect delay. */
  reopenMs: number;
};

export function createSubscriptions({
  page,
  encodeCommand,
  withCheckout,
  commandFrame,
  reopenMs,
}: SubscriptionDeps) {
  // A subscription on a channel of `kind` of the page socket. Each open of
  // the socket opens the channel again and runs `onOpen`; `close` ends the
  // channel and leaves the socket. A channel the daemon ends while its socket
  // stays open is opened again after `reopenMs`, and runs `onOpen` as a
  // reopen. It is opened again once per socket: a second end waits for the
  // next open of the socket.
  function channelSubscription(kind: ChannelKind, { onOpen, onPush }: ChannelHandlers) {
    let channel: Channel | null = null;
    let closed = false;
    // Counts the opens of the socket, so a timer of an older socket does nothing.
    let socketOpens = 0;
    let reopenedOnce = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const attach = (channels: PageChannels, reopened: boolean) => {
      const ch = channels.open(kind, {});
      // A socket that is not open gives a closed channel; its next open attaches.
      if (ch.readyState === "closed") return;
      channel = ch;
      ch.onFrame = (inner: Uint8Array) => {
        const f = commandFrame(inner);
        if (f) onPush(f);
      };
      ch.onClosed = (ended: CloseReason) => {
        if (ended !== "socket-lost") reopenLater(channels);
      };
      onOpen((frame) => ch.send(frame), reopened);
    };
    const reopenLater = (channels: PageChannels) => {
      if (closed || reopenedOnce) return;
      reopenedOnce = true;
      const opens = socketOpens;
      timer = setTimeout(() => {
        if (!closed && opens === socketOpens) attach(channels, true);
      }, reopenMs);
    };
    const leave = page.join({
      onOpen: (channels: PageChannels, reopened: boolean) => {
        socketOpens += 1;
        reopenedOnce = false;
        clearTimeout(timer);
        attach(channels, reopened);
      },
    });
    return {
      sendIfOpen: (frame: Uint8Array) => channel?.send(frame) ?? false,
      resume: page.resume,
      close: () => {
        closed = true;
        clearTimeout(timer);
        channel?.close();
        channel = null;
        leave();
      },
    };
  }

  // Open ONE `tree` channel for a project (#196, ADR-0036 §4):
  // `watch`/`unwatch` a rel dir as the tree expands/collapses, and invoke
  // `onDirty(relPath)` for each `tree.dirty` push. Returns the control handle; the
  // caller closes it when the project closes (the daemon tears the watcher down on
  // the last release).
  // A subscription is bound to ONE checkout (#406): every watch carries it and
  // a `tree.dirty` for another tree of the same repo is not this tree's news —
  // the caller remounts the tree (and this subscription) when the selection
  // changes, so the filter only ever drops a frame from a stale watch.
  // The same channel holds the checkout's HEAD (`head.watch`): a branch switch or
  // a commit made anywhere pushes `head.dirty`, and `onHead()` re-reads the
  // branch. The channel's close releases that hold with the tree's.
  // The held dirs are STATE, not a queue: a new channel starts empty on the
  // daemon, so every open sends `head.watch` and each held `watch` again, and a
  // RE-open re-reads each held dir and the branch once — whatever changed while
  // the socket was down was never pushed (#484).
  // A `tree.failed` push says the daemon could not watch a dir of this tree:
  // `onFailed(reason)`. A reopen holds every dir again: `onFailed(null)`.
  function subscribeTree(
    repo: string,
    onDirty: (path: string) => void,
    checkout: string | null | undefined,
    onHead: (() => void) | undefined,
    onFailed: ((reason: string | null) => void) | undefined,
  ) {
    const held = new Set<string>();
    const frame = (verb: string, path: string) =>
      encodeCommand({ id: 0, verb, payload: withCheckout({ repo, path: path || "" }, checkout) });
    // Re-reads each held dir and the branch: a reopen does it, and so does a
    // tab that becomes visible or logs in (ADR-0070 D2 events 3, 4).
    const replay = () => {
      for (const path of held) onDirty(path);
      onHead?.();
    };
    const sub = channelSubscription("tree", {
      onOpen: (send, reopened) => {
        if (onHead) send(frame("head.watch", ""));
        for (const path of held) send(frame("watch", path));
        if (reopened) {
          // A new channel holds every dir again, so a failed watch is cleared.
          onFailed?.(null);
          replay();
        }
      },
      onPush: (f) => {
        if (!["tree.dirty", "head.dirty", "tree.failed"].includes(f.verb)) return;
        const p = f.payload || {};
        if ((p.checkout || null) !== (checkout || null)) return;
        treePush(f.verb, p);
      },
    });
    // A push of this checkout's tree: a dir it could not watch, a moved HEAD,
    // or a changed dir.
    function treePush(verb: string, p: PushPayload) {
      if (verb === "tree.failed") onFailed?.(p.reason || "");
      else if (verb === "head.dirty") onHead?.();
      else onDirty(p.path || "");
    }
    return {
      watch: (path: string) => {
        const rel = path || "";
        if (held.has(rel)) return;
        held.add(rel);
        sub.sendIfOpen(frame("watch", rel));
      },
      // While disconnected the set is all there is to change: the next channel
      // never learns the dir.
      unwatch: (path: string) => {
        const rel = path || "";
        if (!held.delete(rel)) return;
        sub.sendIfOpen(frame("unwatch", rel));
      },
      replay,
      resume: sub.resume,
      close: sub.close,
    };
  }

  // Open ONE `runs` channel for a project (#300, ADR-0047 §9): `runs.watch`
  // holds the repo's `.ralphy/runstate` dir and each
  // `runs.dirty` push invokes `onDirty()`, which re-reads `runs.list`. A snapshot
  // is STATE, so the read is idempotent — hence the extra `onDirty()` on every
  // RE-open, the catch-up read that recovers a daemon restart with no operator
  // action. The FIRST open deliberately skips it: a subscription is only ever
  // mounted by a caller that reads the same snapshot itself in the same tick, so
  // catching up there re-read state nobody had yet missed — and this read spawns
  // a CLI, so the duplicate was the dominant cost of opening a project. Catching
  // up is for what arrived while we were DISCONNECTED, which the first connection
  // has no window for.
  function subscribeRuns(repo: string, onDirty: () => void) {
    const sub = channelSubscription("runs", {
      onOpen: (send, reopened) => {
        send(encodeCommand({ id: 0, verb: "runs.watch", payload: { repo, path: "" } }));
        if (reopened) onDirty();
      },
      onPush: (f) => {
        if (f.verb === "runs.dirty") onDirty();
      },
    });
    return { resume: sub.resume, close: sub.close };
  }

  // Open ONE `changes` channel for a project (#310, ADR-0036 amendment).
  // Unlike `subscribeRuns` this sends NO watch frame:
  // `changes.dirty` is not watcher-fed, so every channel receives every nudge
  // and the repo filter is the caller's (`WBChanges.shouldReload`). Each decoded
  // frame is handed over whole — a RE-open synthesizes one for this repo as the
  // catch-up read that recovers a daemon restart. The first open skips it for the
  // reason given on `subscribeRuns`: the mounting caller reads `changes.list` and
  // `sync.status` itself, and each of those spawns the `ralphy` CLI, which spawns
  // `git` — so the synthetic frame doubled the two most expensive reads of
  // opening a project.
  function subscribeChanges(repo: string, onFrame: (frame: PushFrame) => void) {
    const sub = channelSubscription("changes", {
      onOpen: (_send, reopened) => {
        if (reopened) onFrame({ verb: "changes.dirty", payload: { repo } });
      },
      onPush: onFrame,
    });
    return { resume: sub.resume, close: sub.close };
  }

  return { subscribeTree, subscribeRuns, subscribeChanges };
}
