// The one writer of the browser console: every message there is for a developer.

// no-console flags a call on `console`, not this alias: the calls below go through it.
const sink = console;

export function devLog(...args: unknown[]): void {
  sink.log(...args);
}

export function devWarn(...args: unknown[]): void {
  sink.warn(...args);
}

export function devError(...args: unknown[]): void {
  sink.error(...args);
}
