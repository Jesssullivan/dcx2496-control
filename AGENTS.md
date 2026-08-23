# DCX2496 Control Agent Contract

This repository owns the typed, deterministic DCX2496 protocol and profile
runtime. Legalab owns the cross-repository studio ontology and PZM activation.

- Use `just` as the operator entrypoint and Bazel labels as the build graph.
- `dcxctl` is offline-only. The sole candidate hardware-capable boundary is the
  review-gated Darwin carrier in `dcx-darwin-tty`: it accepts one private digest-bound
  callout path and can emit only the typed Search query. Live use still requires
  Legalab's attended hardware gate; no enumeration or generic write is allowed.
- Unknown identity, state, route, or value is an error. Never infer that an
  output is safe or muted.
- Tests use synthetic or explicitly sanitized fixtures. Never commit captures,
  device serials, credentials, proprietary software, or calibration files.
- Preserve source provenance and per-file licenses. Do not claim an observed
  protocol fact from a synthetic fixture.
- Use pull requests and run `just check` before claiming completion.
- Never add AI attribution or `Co-Authored-By` trailers.
