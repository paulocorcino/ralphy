# The vendored docx-preview module

The document viewer draws a docx with docx-preview (ADR-0091, amendment of
2026-11-23). This recipe bundles docx-preview and jszip into one ES module,
`../../assets/ui/vendor/docx-preview/docx-preview.js`.

```sh
npm ci && node build.mjs
```

Run it by hand when a pin in `package.json` moves. It does not run in CI:
the output is committed. Then update the sha256 values in
`assets/ui/vendor/manifest.json`.

After a bump, check the output again: no `eval`, and the one `new Function`
is jszip's `setImmediate` shim, which runs only when given a string.
