/* ---------------------------------------------------------------------------
   ralphy workbench — the detach LINK seam

   `wb-console.js` decides WHICH fences are detached and WHEN a peer is lost;
   this module owns the two browser facilities that carry those facts across a
   reload and between two documents:

     the registry   `sessionStorage["wb.detach.v1"]` — scoped to ONE tab, so it
                    survives that tab's F5 and dies with the tab (ADR-0051 §8)
     the channel    `new BroadcastChannel("wb.detach.v1")` — same-origin by
                    specification, which is why the lifecycle chatter needs no
                    `targetOrigin` of its own; the initial handover keeps
                    #346's concrete-`targetOrigin` postMessage handshake

   Two implementations, one surface, exactly as `wb-desk-sink.js`:

     link()   the shell's real link — storage + channel + a tab identity
     none()   inert, `tab: null`, an empty registry that never writes

   The seam exists because the same console module runs in the detached-fence
   popup, and `window.open` hands that popup a COPY of the opener's
   sessionStorage. A registry read there would look plausible and be a ghost:
   the popup is not the acting tab, and a stale copy of its registry drifts the
   moment the origin writes. Injecting `none()` makes the popup INCAPABLE of
   acting on a registry rather than merely careful about it.

   The channel is browser-WIDE — a second tab of the same origin hears every
   message — so every message carries the acting tab's id and both sides drop
   anything whose `tab` differs. The popup learns that id from the handover
   payload, never from its own (copied) storage.

   INVARIANT: nothing at module load touches `sessionStorage`, a
   `BroadcastChannel` or a timer. The node harness loads this file with neither
   present, and so does a browser in private mode.
--------------------------------------------------------------------------- */
window.WBDetachLink = (function () {
  const KEY = "wb.detach.v1";
  const CHANNEL = "wb.detach.v1";
  // One beat per second, six seconds of silence before a peer is declared lost.
  // A workbench reload takes ~1-2 s, so an F5 never crosses the window — which
  // is the whole point of the slice: a refresh must not close the popup.
  const HEARTBEAT_MS = 1000;
  const PEER_WINDOW_MS = 6000;

  // Every storage touch is wrapped: private mode throws on `getItem`, a full
  // quota throws on `setItem`, and a hand-edited value parses to anything at
  // all. A detach registry is never worth an exception on a boot path.
  function readRecord() {
    try {
      const raw = sessionStorage.getItem(KEY);
      if (!raw) return null;
      const rec = JSON.parse(raw);
      if (!rec || rec.v !== 1) return null;
      const members = {};
      for (const [id, ids] of Object.entries(rec.members || {})) {
        if (Array.isArray(ids)) members[id] = ids.filter((w) => typeof w === "string");
      }
      return {
        v: 1,
        tab: typeof rec.tab === "string" ? rec.tab : null,
        fences: Array.isArray(rec.fences) ? rec.fences.filter((f) => typeof f === "string") : [],
        members,
      };
    } catch {
      return null;
    }
  }

  function writeRecord(rec) {
    try {
      sessionStorage.setItem(KEY, JSON.stringify(rec));
    } catch {}
  }

  function newTabId() {
    // Same hand-rolled shape as `wb-console.js`'s `newId`: `crypto.randomUUID`
    // is undefined in a non-secure context and the daemon can bind plain http.
    return "t-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 10);
  }

  // The channel half, alone. `undefined` = not yet asked, `null` = asked and
  // unavailable — created on FIRST USE so the module load stays free of every
  // browser facility.
  function makeChannel() {
    let chan;
    const listeners = [];
    function open() {
      if (chan !== undefined) return chan;
      chan = null;
      if (typeof BroadcastChannel !== "undefined") {
        try {
          chan = new BroadcastChannel(CHANNEL);
          chan.onmessage = (e) => {
            for (const fn of listeners.slice()) {
              try {
                fn(e.data);
              } catch {}
            }
          };
        } catch {
          chan = null;
        }
      }
      return chan;
    }
    return {
      post(msg) {
        const c = open();
        if (!c) return;
        try {
          c.postMessage(msg);
        } catch {}
      },
      onMessage(fn) {
        if (typeof fn !== "function") return;
        listeners.push(fn);
        open();
      },
      close() {
        listeners.length = 0;
        if (chan) {
          try {
            chan.close();
          } catch {}
        }
        chan = null;
      },
    };
  }

  // The popup's link: the lifecycle channel and NOTHING else. It carries no
  // registry at all — not even a read — because `window.open` handed that
  // document a COPY of its opener's store, so every answer it could give is a
  // ghost. Its `tab` is null: the popup is told its opener's id in the handover.
  function channel() {
    return { tab: null, HEARTBEAT_MS, PEER_WINDOW_MS, ...makeChannel() };
  }

  function link() {
    const rec = readRecord();
    // The id is minted ONCE per tab and persisted BESIDE the registry, by
    // `writeRegistry` — never at load. A tab that detaches nothing must leave
    // the store untouched: the key's very presence is the fact "this tab has a
    // detach", and a boot-time write would make every tab claim one.
    const tab = rec?.tab || newTabId();
    const chan = makeChannel();

    return {
      tab,
      HEARTBEAT_MS,
      PEER_WINDOW_MS,
      ...chan,
      // Read through to storage every time rather than from a cached array: a
      // second document of the same tab (the popup's copy) must never be able
      // to hand this one a stale registry.
      readRegistry() {
        return readRecord()?.fences || [];
      },
      // The member ids each detached fence held, `{ fenceId: [windowId] }`.
      // Stored rather than re-derived from geometry at boot: a detached fence
      // may still be MOVED on the plane (#346, ADR-0051 §7a) while its members'
      // records keep the rects they were detached at, so a membership fold run
      // after a reload would answer "no members" and put them back on the plane
      // under a live popup.
      readMembers() {
        return readRecord()?.members || {};
      },
      writeRegistry(ids, members) {
        writeRecord({
          v: 1,
          tab,
          fences: Array.isArray(ids) ? ids.slice() : [],
          members: members && typeof members === "object" ? members : {},
        });
      },
    };
  }

  // The popup's link: the same surface, incapable of reading or writing a
  // registry and carrying no tab identity of its own (it is handed its opener's).
  function none() {
    return {
      tab: null,
      HEARTBEAT_MS,
      PEER_WINDOW_MS,
      readRegistry() {
        return [];
      },
      readMembers() {
        return {};
      },
      writeRegistry() {},
      post() {},
      onMessage() {},
      close() {},
    };
  }

  return { link, channel, none, KEY, CHANNEL, HEARTBEAT_MS, PEER_WINDOW_MS };
})();
