/* ---------------------------------------------------------------------------
   wb-read-retry.ts — a read that is lost on the way is asked again while the
   caller keeps its spinner. Pure: no DOM, so a test imports it as it is.
   --------------------------------------------------------------------------- */

// A reply that said no (`status: "error"`). Asking again gets the same answer,
// so it is never retried. A read that failed WITHOUT a reply (a socket that
// closed, a link that dropped) says nothing about the directory: a phone on a
// mobile link loses such a read now and then.
export class Refused extends Error {}

export const ATTEMPTS = 3;
export const RETRY_DELAY_MS = 1500;

const pause = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

// Run `read` up to `ATTEMPTS` times, waiting `RETRY_DELAY_MS` between the
// tries. A `Refused` error, or the last try failing, rejects with that error.
export async function patiently<T>(
  read: () => Promise<T>,
  wait: (ms: number) => Promise<void> = pause,
): Promise<T> {
  for (let attempt = 1; ; attempt++) {
    try {
      return await read();
    } catch (err) {
      if (err instanceof Refused || attempt >= ATTEMPTS) throw err;
      await wait(RETRY_DELAY_MS);
    }
  }
}

export const WBReadRetry = { Refused, patiently };
