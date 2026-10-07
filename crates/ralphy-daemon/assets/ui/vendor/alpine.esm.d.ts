// The types of the vendored Alpine ES module build (ADR-0075 D4): the
// surface `globals.d.ts` gives `window.Alpine`, which `main.ts` sets.
declare const Alpine: Window["Alpine"];
export default Alpine;
