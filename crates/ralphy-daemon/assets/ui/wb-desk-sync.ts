// The desk as this page knows it (ADR-0050 amendment 2026-10-04, changes, not
// the desk): the daemon's last desk, the desk changes this page made that the
// daemon has not answered yet, and the view that draws the two together.
//
// PURE: data in, data out. No DOM, no fetch, no timers. `wb-console.ts` emits
// a change for each act, sends `nextBatch()` through the desk sink, and hands
// every desk the daemon serves to `take`.
//
// `applyChange` is the daemon's `desk::apply` rule, in JavaScript. One table
// of cases, `ui-tests/fixtures/api-desk--apply-cases.json`, runs on both
// sides, so the two cannot drift apart.
export const WBDeskSync = (function () {

  // The record types; each lives in the collection named by its plural.
  const TYPES = new Set(["window", "fence", "note"]);

  function emptyDesk() {
    return { windows: [], fences: [], notes: [], checkouts: {} };
  }

  // A served desk as the four collections, each a fresh array: a page never
  // mutates what the daemon sent.
  function deskOf(payload: any) {
    const list = (v: any) => (Array.isArray(v) ? v.slice() : []);
    const map = payload?.checkouts;
    return {
      windows: list(payload?.windows),
      fences: list(payload?.fences),
      notes: list(payload?.notes),
      checkouts: map && typeof map === "object" && !Array.isArray(map) ? { ...map } : {},
    };
  }

  // Absent, `null` and `false` are one value on the daemon (an `Option` or a
  // `bool` that is not serialised), so they are one value here.
  const none = (v: any) => v == null || v === false;
  function same(a: any, b: any) {
    if (none(a) || none(b)) return none(a) && none(b);
    if (typeof a === "object" || typeof b === "object") {
      return JSON.stringify(a) === JSON.stringify(b);
    }
    return a === b;
  }

  // Set `key` on `rec` (a copy) to `value`; whether it changed. `null` and
  // `false` delete the key, the shape the daemon serves.
  function put(rec: any, key: any, value: any) {
    if (same(rec[key], value)) return false;
    if (value == null || value === false) delete rec[key];
    else rec[key] = value;
    return true;
  }

  function rectOf(r: any) {
    return { left: r.left, top: r.top, width: r.width, height: r.height };
  }

  // The fields of one `set`, per record type. The daemon refuses any other.
  function setFields(type: any, rec: any, fields: any) {
    let did = false;
    if (fields.rect) {
      const r = rectOf(fields.rect);
      if (!rec.rect || !same(rectOf(rec.rect), r)) {
        rec.rect = r;
        did = true;
      }
    }
    if (typeof fields.locked === "boolean") did = put(rec, "locked", fields.locked) || did;
    if (type === "window") {
      if (typeof fields.max === "boolean") {
        // `max` is always written, like the daemon's `bool` without a skip.
        if (!!rec.max !== fields.max) {
          rec.max = fields.max;
          did = true;
        }
      }
      // A blank name is no name: the stored one stays.
      if (typeof fields.consoleName === "string" && fields.consoleName.trim()) {
        did = put(rec, "consoleName", fields.consoleName) || did;
      }
      if ("checkout" in fields) did = put(rec, "checkout", fields.checkout ?? null) || did;
      if (fields.session) {
        const s = fields.session;
        if (!same(rec.sessionId, s.sessionId ?? null)) {
          rec.sessionId = s.sessionId ?? null;
          did = true;
        }
        did = put(rec, "daemonId", s.daemonId ?? null) || did;
        did = put(rec, "environment", s.environment ?? null) || did;
      }
    } else if (type === "fence") {
      if (typeof fields.name === "string" && fields.name !== (rec.name ?? "")) {
        rec.name = fields.name;
        did = true;
      }
    } else if (type === "note" && fields.file) {
      const f = fields.file;
      if (f.repo !== rec.repo) {
        rec.repo = f.repo;
        did = true;
      }
      if ((f.path ?? "") !== (rec.path ?? "")) {
        rec.path = f.path ?? "";
        did = true;
      }
      did = put(rec, "checkout", f.checkout ?? null) || did;
    }
    return did;
  }

  // One change onto a desk: `{ desk, changed }`, never mutating `desk`.
  function applyChange(desk: any, change: any) {
    const unchanged = { desk, changed: false };
    const op = change?.op;
    if (op === "checkout" || op === "checkout-clear") {
      const held = desk.checkouts[change.repo];
      if (op === "checkout" ? held === change.name : held !== change.ifName) return unchanged;
      const checkouts = { ...desk.checkouts };
      if (op === "checkout") checkouts[change.repo] = change.name;
      else delete checkouts[change.repo];
      return { desk: { ...desk, checkouts }, changed: true };
    }
    if (!TYPES.has(change?.type)) return unchanged;
    const key = `${change.type}s`;
    const list = desk[key];
    const id = op === "create" ? change.record?.id : change.id;
    const at = list.findIndex((r: any) => r.id === id);
    let next;
    if (op === "create") {
      if (at < 0) {
        next = list.concat([{ ...change.record }]);
      } else {
        // A page that adopted a running console: only its session is news.
        if (change.type !== "window") return unchanged;
        const rec = { ...list[at] };
        const r = change.record;
        const did = setFields("window", rec, {
          session: { sessionId: r.sessionId, daemonId: r.daemonId, environment: r.environment },
        });
        if (!did) return unchanged;
        next = list.slice();
        next[at] = rec;
      }
    } else if (op === "set") {
      // Deleted elsewhere: a change must not bring it back.
      if (at < 0) return unchanged;
      const rec = { ...list[at] };
      if (!setFields(change.type, rec, change.fields || {})) return unchanged;
      next = list.slice();
      next[at] = rec;
    } else if (op === "remove") {
      if (at < 0) return unchanged;
      next = list.filter((r: any) => r.id !== id);
    } else {
      return unchanged;
    }
    return { desk: { ...desk, [key]: next }, changed: true };
  }

  // The daemon's desk with this page's pending changes on top.
  function overlay(server: any, pending: any) {
    return pending.reduce((d: any, c: any) => applyChange(d, c).desk, server);
  }

  // The state of one page's desk. `phase` is one of
  //   "loading"  — no desk read yet; changes queue, nothing is sent;
  //   "ready"    — a desk was read; changes are sent;
  //   "failed"   — the daemon cannot read its desk; changes queue;
  //   "restored" — a restore happened since this page read the desk; the page
  //                reloads and sends nothing more.
  function createSync() {
    let phase = "loading";
    let rev: any = null;
    let generation: any = null;
    let seq = 0;
    let server: any = emptyDesk();
    // Unanswered changes, in order. The first `inflight.count` are the batch
    // the daemon has not answered yet; a resend reuses its `seq`.
    let pending: any = [];
    let inflight: any = null;

    return {
      phase: () => phase,
      generation: () => generation,
      rev: () => rev,
      view: () => overlay(server, pending),
      hasPending: () => pending.length > 0,
      // Take a desk the daemon served: a GET, or the reply to a PUT.
      // Returns "taken", "stale" (an older `rev` than one already taken),
      // "reload" (a restore happened since this page read the desk) or
      // "ignored" (the page is reloading).
      take(payload: any) {
        if (phase === "restored") return "ignored";
        const gen = Number(payload?.generation) || 0;
        if (generation != null && gen > generation) {
          phase = "restored";
          return "reload";
        }
        const r = Number(payload?.rev) || 0;
        if (rev != null && r < rev) return "stale";
        rev = r;
        generation = gen;
        server = deskOf(payload);
        phase = "ready";
        return "taken";
      },
      fail() {
        if (phase !== "restored") phase = "failed";
      },
      restored() {
        phase = "restored";
      },
      emit(change: any) {
        pending.push(change);
      },
      // The body to send now: the batch in flight again (same `seq`), else
      // every pending change as a new batch. Null with nothing to send.
      nextBatch() {
        if (!inflight) {
          if (!pending.length) return null;
          inflight = { seq: ++seq, count: pending.length };
        }
        return {
          seq: inflight.seq,
          generation: generation || 0,
          changes: pending.slice(0, inflight.count),
        };
      },
      // The daemon answered the batch in flight with `reply`, its desk.
      acked(reply: any) {
        if (inflight) pending.splice(0, inflight.count);
        inflight = null;
        return reply ? this.take(reply) : "taken";
      },
      // The daemon will never accept the batch in flight (a 400 or a 422).
      dropped() {
        if (inflight) pending.splice(0, inflight.count);
        inflight = null;
      },
      // The page is closing: every pending change, as a new batch with a
      // higher `seq`, so a batch still on the wire that lands after it is
      // ignored and nothing is applied twice.
      closingBatch() {
        if (!pending.length) return null;
        return { seq: ++seq, generation: generation || 0, changes: pending.slice() };
      },
    };
  }

  return { applyChange, overlay, createSync, emptyDesk };
})();
