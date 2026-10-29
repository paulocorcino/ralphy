/* ---------------------------------------------------------------------------
   wb-api.ts — the daemon's HTTP routes that the page reads a JSON reply from,
   and `apiFetch`, the one way to call them (#653).

   The daemon and the page ship in one build, so a reply is not checked at run
   time. Instead `ApiRoutes` lists each route ONCE, with the body the daemon
   sends on success (`ok`) and, when the page reads it, on a refusal
   (`failed`). Each type follows the route's handler in
   `crates/ralphy-daemon/src/routes/`: an `Option` is `T | null`, and a field
   with `skip_serializing_if` is optional. A route that sends a refusal as
   plain text has no `failed`: its body is `unknown`.

   The key is `"<METHOD> <path>"`, because one path can answer two methods
   with two bodies (`GET` and `PUT /api/desk`). A key that ends in `?<name>`
   is a path whose reply depends on that query parameter
   (`GET /api/desk/history?id`): the caller passes it in `query`.

   A socket frame is read through `socketFrame`, for the same reason. Other
   data from outside (another window's message, stored data, an uploaded
   file) is `unknown`, and is narrowed where it is read.
   --------------------------------------------------------------------------- */
import type { FleetPeer } from "./wb-fleet.ts";
import type { Desk, DeskReply } from "./wb-types.d.ts";
import type { MissingUsage, SpendDoc, SpendRecord } from "./wb-spend.ts";
import type { DeskVersion, VersionInfo } from "./wb-desk-history.ts";
import type { AuditEvent, Device } from "./wb-devices.ts";
import type { ReleaseView } from "./wb-release.ts";
import type { AttentionReply, TelegramReply } from "./wb-attention.ts";
import { reportStall } from "./wb-stall.ts";

/** What HEAD points at (`ralphy_git_read::Head`). */
export type Head = { kind: "branch"; name: string } | { kind: "detached"; sha: string };

/** A repo of `/api/repos` (`RepoView` in `routes/api_read.rs`). */
export type RepoRow = {
  slug: string;
  name: string;
  path: string;
  reachable: boolean;
  /** Set only on a branch; `head` tells a detached HEAD from no answer. */
  branch: string | null;
  head: Head | null;
  dirty: boolean;
  remote: string | null;
  /** The absolute native root. */
  root: string | null;
};

/** A repo of `/api/fleet`: the local ones (`local`) and each peer's
 * (`FederatedRepo` in `fleet.rs`). */
export type FleetRepoRow = {
  key: string;
  daemon_id: string;
  daemon_name: string;
  environment: string;
  os: string;
  peer_state: string;
  slug: string;
  name: string;
  path: string;
  reachable: boolean;
  branch: string | null;
  /** `null` on a local row: `/api/repos` reads it. */
  dirty: boolean | null;
  remote: string | null;
  /** The peer's absolute native root; `null` on a local row, and on a peer
   * that predates the field. */
  root?: string | null;
  local: boolean;
};

/** The reply of `/api/fleet`. */
export type FleetReply = { peers: FleetPeer[]; repos: FleetRepoRow[] };

/** A row of the adapter roster (`AgentRow` in `roster.rs`). */
type AgentRow = { id: string; label: string; accelerator: string; available: boolean; reason: string | null };

/** A refusal that names its cause. */
type ErrorBody = { error: string };

/** Every route the page reads a JSON reply from. */
interface ApiRoutes {
  "GET /api/identity": { ok: { name: string; avatar: string } };
  /** Reachable before login. */
  "GET /api/session": {
    ok: { authed: boolean; password: boolean; policy: "localhost" | "bearer" | "session"; avatar?: string };
  };
  "GET /api/about": { ok: { name: string; version: string; license: string; repository: string; creator: string } };
  /** With `repo`, a peer's roster: the daemon passes the peer's body on as it came. */
  "GET /api/agents": { ok: AgentRow[] };
  "GET /api/repos": { ok: RepoRow[] };
  "GET /api/fleet": { ok: FleetReply };
  /** `200` whether the peer woke or not: `ready` says which. */
  "POST /api/fleet/nudge": {
    ok: { nudged: true; ready: boolean; waited_ms: number; state: string; diagnosis: string };
    failed: ErrorBody;
  };
  /** A command with one reply: `200` with the reply, a refusal included. */
  "POST /api/command": { ok: DaemonReply };
  "GET /api/sessions": { ok: HostedSession[] };
  /** The names of the peers that did not answer are in `x-ralphy-unanswered`. */
  "GET /api/attention": { ok: AttentionReply };
  /** `409` for a request that still waits, `404` for an unknown id. */
  "POST /api/attention/dismiss": { ok: { dismissed: true } };
  /** This daemon's Telegram state, read from its config at each request. */
  "GET /api/telegram": { ok: TelegramReply };
  "GET /api/spend": { ok: SpendDoc };
  "GET /api/usage": {
    ok: { daemon_id: string | null; records: SpendRecord[]; interactive: SpendRecord[]; missing: MissingUsage[] };
  };
  "GET /api/desk": { ok: Desk; failed: { state: "unreadable" | "unavailable"; error: string } };
  "PUT /api/desk": { ok: DeskReply; failed: DeskReply };
  /** `409 {"state":"readable"}`: another page started the new desk first. */
  "POST /api/desk/new": { ok: { moved_to: string }; failed: { state: "readable" } | ErrorBody };
  "GET /api/desk/history": { ok: VersionInfo[]; failed: ErrorBody };
  "GET /api/desk/history?id": { ok: DeskVersion; failed: ErrorBody };
  "POST /api/desk/history": { ok: { generation: number }; failed: ErrorBody };
  "GET /api/audit/devices": { ok: { devices: Device[] }; failed: ErrorBody };
  "GET /api/audit/events": { ok: { events: AuditEvent[] }; failed: ErrorBody };
  "GET /api/release": { ok: ReleaseView };
  "POST /api/release/watch": { ok: { enabled: boolean } };
  "GET /api/security/state": {
    ok: { token_set: boolean; password_set: boolean; totp_enrolled: boolean; require_login: boolean; remote_images: boolean };
  };
  "POST /api/security/totp/enroll": { ok: { uri: string; newly_minted: boolean } };
  "POST /api/security/totp/confirm": { ok: { confirmed: boolean } };
  "POST /api/security/password": { ok: { password_set: boolean } };
}

/** A route of `ApiRoutes`. */
export type ApiRoute = keyof ApiRoutes;

/** A `2xx` answer: `json()` reads the route's `ok` body. */
export interface ApiOk<T> extends Response {
  readonly ok: true;
  json(): Promise<T>;
}

/** Any other answer: `json()` reads the route's `failed` body. */
export interface ApiFailed<E> extends Response {
  readonly ok: false;
  json(): Promise<E>;
}

type FailedOf<R> = R extends { failed: infer E } ? E : unknown;

/** The answer to `route`. `if (r.ok)` tells which body `json()` reads. */
export type ApiResponse<K extends ApiRoute> = ApiOk<ApiRoutes[K]["ok"]> | ApiFailed<FailedOf<ApiRoutes[K]>>;

/** A refusal of `route`: an answer that is not `2xx`. */
export type ApiRefusal<K extends ApiRoute> = ApiFailed<FailedOf<ApiRoutes[K]>>;

/** What `apiFetch` sends besides the route. The method comes from the route.
 * `deadlineMs` replaces the route's read deadline. `what` names the read in
 * `ReadDeadline` and in the stall report, when the route is not the name the
 * operator knows (a command is named by its verb). */
export type ApiInit = Omit<RequestInit, "method"> & {
  query?: Record<string, string | number>;
  deadlineMs?: number;
  what?: string;
};

/** A read of the page that got no answer by its deadline. `what` names the
 * read (a route, or a command verb) and `waitedMs` is the time it waited. */
export class ReadDeadline extends Error {
  what: string;
  waitedMs: number;
  constructor(what: string, waitedMs: number) {
    super(`no answer to ${what} after ${waitedMs} ms`);
    this.name = "ReadDeadline";
    this.what = what;
    this.waitedMs = waitedMs;
  }
}

/** How long a read may wait for its answer, unless the route has its own. */
export const READ_DEADLINE_MS = 20000;

/** The route that waits on purpose: a nudge waits up to 30 s for the peer to be ready. */
export const ROUTE_DEADLINE_MS: Partial<Record<ApiRoute, number>> = { "POST /api/fleet/nudge": 40000 };

/** The method and the URL of `route` with `query`. Values are encoded; names are not. */
export function apiRequest(route: ApiRoute, query?: Record<string, string | number>) {
  const space = route.indexOf(" ");
  const path = route.slice(space + 1).split("?")[0];
  const pairs = Object.entries(query || {}).map(([name, value]) => `${name}=${encodeURIComponent(value)}`);
  return { method: route.slice(0, space), url: pairs.length ? `${path}?${pairs.join("&")}` : path };
}

// Reject with `ReadDeadline` when `work` has not settled in `ms`. The timer is
// `setTimeout` (not `AbortSignal.timeout`) and is cleared as soon as `work`
// settles, so a fast answer leaves no pending timer.
function beforeDeadline<T>(work: Promise<T>, what: string, ms: number, onDeadline: () => void): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      onDeadline();
      reject(new ReadDeadline(what, ms));
    }, ms);
    work.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

// The body read has its own deadline of the same length: `json()` and `text()`
// of a reply that stops half way would wait for ever. A reply with no such
// method (a test double) is returned as it is.
function boundBodyReads(res: Response, what: string, ms: number, abort: () => void) {
  for (const name of ["json", "text"] as const) {
    const read = res[name]?.bind(res);
    if (typeof read !== "function") continue;
    Object.defineProperty(res, name, {
      configurable: true,
      value: () => beforeDeadline(read(), what, ms, abort),
    });
  }
}

/** Call `route`. A transport failure rejects, as `fetch` does; a refusal
 * resolves, and the caller reads `ok` and `status`. A request, or the read of
 * its body, with no answer by its deadline rejects with `ReadDeadline`. */
export function apiFetch<K extends ApiRoute>(route: K, init: ApiInit = {}): Promise<ApiResponse<K>> {
  const { query, deadlineMs, what: named, signal: callerSignal, ...rest } = init;
  const what = named ?? route;
  const { method, url } = apiRequest(route, query);
  const ms = deadlineMs ?? ROUTE_DEADLINE_MS[route] ?? READ_DEADLINE_MS;
  const controller = new AbortController();
  const abort = () => controller.abort();
  const stalled = () => {
    abort();
    void reportStall("read-deadline", what, ms);
  };
  if (callerSignal) {
    if (callerSignal.aborted) abort();
    else callerSignal.addEventListener("abort", abort, { once: true });
  }
  // The one unchecked step: the reply comes from the daemon of the same
  // build, whose handler `ApiRoutes` follows.
  const sent = fetch(url, { ...rest, method, signal: controller.signal }) as Promise<ApiResponse<K>>;
  // The caller's signal may outlive many requests: drop the listener once
  // this one settles, so it does not keep the controller alive.
  const forget = () => callerSignal?.removeEventListener("abort", abort);
  return beforeDeadline(sent, what, ms, stalled).then(
    (res) => {
      forget();
      boundBodyReads(res, what, ms, stalled);
      return res;
    },
    (e: unknown) => {
      forget();
      throw e;
    },
  );
}

/** The reply of a command answered over `POST /api/command`. The one
 * unchecked step for a command reply, as `socketFrame` is for a frame: the
 * daemon of the same build writes the reply, and `ReplyOf<V>` follows its verb. */
export function commandReply<V extends string>(r: ApiOk<DaemonReply>): Promise<ReplyOf<V>> {
  return r.json() as Promise<ReplyOf<V>>;
}

/** The JSON after the tag byte of a `[tag][JSON]` frame on a socket of this
 * daemon (`src/protocol.rs`). It throws on bytes that are not JSON. */
export function socketFrame<T>(bytes: Uint8Array): T {
  // The one unchecked step for a frame, as the `fetch` above is for a reply.
  // The daemon of the same build writes the frame. A frame or a reply that a
  // peer daemon wrote is relayed only from a peer that speaks this daemon's
  // peer protocol version, and the daemon refuses a peer of another version.
  // The same version means the same reply shapes, because a change to a type
  // that reads a peer's reply fails `peer_reply_types_are_pinned_to_the_protocol_version`
  // (crates/xtask/tests/ratchets/) until the version is raised, or until
  // each change is a new field declared optional (#653).
  return JSON.parse(new TextDecoder().decode(bytes.subarray(1)));
}

// The checks that narrow data from outside, shared by the modules that read it.

/** Whether `v` is a JSON object: not `null`, not an array. */
export function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Whether `v` is a string or absent. */
export function isOptionalString(v: unknown): v is string | undefined {
  return v === undefined || typeof v === "string";
}

/** Whether `v` is a string or `null`. */
export function isNullableString(v: unknown): v is string | null {
  return v === null || typeof v === "string";
}
