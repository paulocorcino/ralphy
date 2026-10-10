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
  "GET /api/sessions": { ok: HostedSession[] };
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

/** What `apiFetch` sends besides the route. The method comes from the route. */
export type ApiInit = Omit<RequestInit, "method"> & { query?: Record<string, string | number> };

/** The method and the URL of `route` with `query`. Values are encoded; names are not. */
export function apiRequest(route: ApiRoute, query?: Record<string, string | number>) {
  const space = route.indexOf(" ");
  const path = route.slice(space + 1).split("?")[0];
  const pairs = Object.entries(query || {}).map(([name, value]) => `${name}=${encodeURIComponent(value)}`);
  return { method: route.slice(0, space), url: pairs.length ? `${path}?${pairs.join("&")}` : path };
}

/** Call `route`. A transport failure rejects, as `fetch` does; a refusal
 * resolves, and the caller reads `ok` and `status`. */
export function apiFetch<K extends ApiRoute>(route: K, init: ApiInit = {}): Promise<ApiResponse<K>> {
  const { query, ...rest } = init;
  const { method, url } = apiRequest(route, query);
  // The one unchecked step: the reply comes from the daemon of the same
  // build, whose handler `ApiRoutes` follows.
  return fetch(url, { ...rest, method }) as Promise<ApiResponse<K>>;
}

/** The JSON after the tag byte of a `[tag][JSON]` frame on a socket of this
 * daemon (`src/protocol.rs`). It throws on bytes that are not JSON. */
export function socketFrame<T>(bytes: Uint8Array): T {
  // The one unchecked step for a frame, as the `fetch` above is for a reply.
  // The daemon of the same build writes the frame. A frame or a reply that a
  // peer daemon wrote is relayed only from a peer that speaks this daemon's
  // peer protocol version: the daemon refuses a peer of another version.
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
