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
      return {
        v: 1,
        tab: typeof rec.tab === "string" ? rec.tab : null,
        fences: Array.isArray(rec.fences) ? rec.fences.filter((f) => typeof f === "string") : [],
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

  function link() {
    const rec = readRecord();
    // The id is generated ONCE per tab and stored beside the registry, so it
    // survives the reload the registry survives and dies with it.
    const tab = rec?.tab || newTabId();
    if (!rec || rec.tab !== tab) writeRecord({ v: 1, tab, fences: rec?.fences || [] });

    // `undefined` = not yet asked, `null` = asked and unavailable. Created on
    // first use so the module load stays free of browser facilities.
    let chan;
    const listeners = [];
    function channel() {
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
      tab,
      HEARTBEAT_MS,
      PEER_WINDOW_MS,
      // Read through to storage every time rather than from a cached array: a
      // second document of the same tab (the popup's copy) must never be able
      // to hand this one a stale registry.
      readRegistry() {
        return readRecord()?.fences || [];
      },
      writeRegistry(ids) {
        writeRecord({ v: 1, tab, fences: Array.isArray(ids) ? ids.slice() : [] });
      },
      post(msg) {
        const c = channel();
        if (!c) return;
        try {
          c.postMessage(msg);
        } catch {}
      },
      onMessage(fn) {
        if (typeof fn !== "function") return;
        listeners.push(fn);
        channel();
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
      writeRegistry() {},
      post() {},
      onMessage() {},
      close() {},
    };
  }

  return { link, none, KEY, CHANNEL, HEARTBEAT_MS, PEER_WINDOW_MS };
})();
