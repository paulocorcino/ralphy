// The id lookups of a fake document: `find(id)` answers both `getElementById(id)`
// and `querySelector("#id")`; any other selector finds nothing.
export function idLookups(find) {
  return {
    getElementById: find,
    querySelector: (sel) => (sel.startsWith("#") ? find(sel.slice(1)) : null),
  };
}
