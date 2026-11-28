// The fakes and the markup runners of the test harness that read no file;
// harness.mjs exports them again, so a test imports them from there.

// The names a markup binding reads from its scope: every identifier that is not
// a property (`a.b`), an object key (`{ on: … }`), a string, or a keyword.
// Loop variables of `x-for` and `$` magics are left to the caller.
export function bindingNames(expr) {
  const code = expr
    // `&amp;` last, so `&amp;lt;` stays the text `&lt;`.
    .replaceAll('&lt;', "<")
    .replaceAll('&gt;', ">")
    .replaceAll('&amp;', "&")
    .replaceAll(/'(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*"/gu, "''");
  const names = new Set();
  for (const m of code.matchAll(/[A-Za-z_$][\w$]*/gu)) {
    const before = code.slice(0, m.index);
    const after = code.slice(m.index + m[0].length);
    if (/\.\s*$/u.test(before)) continue;
    if (/[{,]\s*$/u.test(before) && /^\s*:/u.test(after)) continue;
    if (["true", "false", "null", "undefined", "in", "of", "typeof"].includes(m[0])) continue;
    names.add(m[0]);
  }
  return [...names];
}

// Run a markup expression the way Alpine does: names resolve on `scope`.
// `with` needs sloppy mode, which `new Function` gives.
// oxlint-disable-next-line eslint/no-new-func -- the test runs markup code, as Alpine does
export const runIn = (scope, expr) => new Function("scope", `with (scope) { ${expr}; }`)(scope);
// oxlint-disable-next-line eslint/no-new-func -- the test runs markup code, as Alpine does
export const valueIn = (scope, expr) => new Function("scope", `with (scope) { return ${expr}; }`)(scope);

// Replace the global `fetch` with `fake` until test `t` ends. Returns the
// calls, as `{ url, init }`, in the order they came.
export function swapFetch(t, fake) {
  const real = globalThis.fetch;
  const calls = [];
  globalThis.fetch = (url, init) => {
    calls.push({ url, init });
    return fake(url, init);
  };
  t.after(() => {
    globalThis.fetch = real;
  });
  return calls;
}

// A window that delivers each event it is sent to its listeners, as the
// browser does, and keeps every event in `events`. `extra` adds members.
export function fakeWindow(extra = {}) {
  const listeners = {};
  const events = [];
  return {
    events,
    addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
    removeEventListener: (type, fn) => {
      listeners[type] = (listeners[type] || []).filter((f) => f !== fn);
    },
    dispatchEvent: (e) => {
      events.push(e);
      for (const fn of listeners[e.type] || []) fn(e);
      return true;
    },
    ...extra,
  };
}

// An element's `classList` over a set of names, with the DOM's answers:
// `toggle` returns whether the name is on afterwards. `has` and `classes`
// let a test read the names without the DOM's longer words.
export function fakeClassList(...names) {
  const set = new Set(names);
  return {
    contains: (c) => set.has(c),
    has: (c) => set.has(c),
    get classes() {
      return set;
    },
    add: (...cs) => cs.forEach((c) => set.add(c)),
    remove: (...cs) => cs.forEach((c) => set.delete(c)),
    toggle: (c, force = !set.has(c)) => (force ? set.add(c) : set.delete(c), force),
    [Symbol.iterator]: () => set.values(),
  };
}

// Run `fn` with no global `BroadcastChannel`. Node 22 ships a real one, the
// console subscribes when it is created, and an open channel holds the event
// loop open, so `node --test` never exits.
export function withoutBroadcast(fn) {
  const real = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    return fn();
  } finally {
    globalThis.BroadcastChannel = real;
  }
}
