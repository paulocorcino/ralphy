// The one writer of the browser console: every message there is for a developer.

// oxlint-disable-next-line no-console -- this module is the one place that writes the console
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
