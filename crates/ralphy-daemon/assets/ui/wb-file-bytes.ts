// The address of a project file's bytes on the daemon's file bytes route
// (ADR-0091 D2): what a viewer gives to `<img src>`, a media element or a
// ranged fetch. `checkout` is the tab's pinned checkout, absent for the
// primary, as `file.read` takes it.
export function fileBytesUrl(repo: string, path: string, checkout: string | null | undefined) {
  const query = new URLSearchParams({ repo, path });
  if (checkout != null && checkout !== "") query.set("checkout", checkout);
  return `/api/file/bytes?${query.toString()}`;
}

// The route answers at most 8 MiB at a time (ADR-0091 D4), so the rest of a
// larger file is asked again from the next byte.
async function readFrom(
  url: string,
  offset: number,
  parts: ArrayBuffer[],
): Promise<ArrayBuffer[] | "too large"> {
  const reply = await fetch(url, { headers: { Range: `bytes=${offset}-` } });
  if (reply.status === 413) return "too large";
  if (!reply.ok) throw new Error(`the file bytes route answered ${reply.status}`);
  const part = await reply.arrayBuffer();
  const next = [...parts, part];
  const end = offset + part.byteLength;
  const total = totalOf(reply.headers.get("Content-Range"));
  if (reply.status !== 206 || part.byteLength === 0 || end >= total) return next;
  return readFrom(url, end, next);
}

// The size after the slash of `bytes 0-8388607/9000000`; 0 when it is absent,
// which ends the reading.
function totalOf(range: string | null) {
  const match = /\/(\d+)$/u.exec(range ?? "");
  return match ? Number(match[1]) : 0;
}

function joined(parts: ArrayBuffer[]) {
  const size = parts.reduce((sum, part) => sum + part.byteLength, 0);
  const out = new Uint8Array(new ArrayBuffer(size));
  let at = 0;
  for (const part of parts) {
    out.set(new Uint8Array(part), at);
    at += part.byteLength;
  }
  return out;
}

/** The whole file at a bytes route address, or "too large" when it is over its cap. */
export async function readWholeFile(url: string): Promise<Uint8Array<ArrayBuffer> | "too large"> {
  const parts = await readFrom(url, 0, []);
  return parts === "too large" ? parts : joined(parts);
}
