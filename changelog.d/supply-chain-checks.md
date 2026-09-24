---
kind: security
headline: 🔒 **Release provenance** — check that a download was built from this repository
---
Each release archive has a signed build provenance: `gh attestation verify
<archive> --repo paulocorcino/ralphy` shows it was built by this repository's
workflow. The TLS library is updated for RUSTSEC-2026-0285.
