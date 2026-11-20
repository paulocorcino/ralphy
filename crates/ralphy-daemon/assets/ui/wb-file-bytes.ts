// The address of a project file's bytes on the daemon's file bytes route
// (ADR-0091 D2): what a viewer gives to `<img src>`, a media element or a
// ranged fetch. `checkout` is the tab's pinned checkout, absent for the
// primary, as `file.read` takes it.
export function fileBytesUrl(repo: string, path: string, checkout: string | null | undefined) {
  const query = new URLSearchParams({ repo, path });
  if (checkout != null && checkout !== "") query.set("checkout", checkout);
  return `/api/file/bytes?${query.toString()}`;
}
