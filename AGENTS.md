# DCX2496 Control Agent Contract

This repository owns the typed, deterministic DCX2496 protocol and profile
runtime. Legalab owns the cross-repository studio ontology and PZM activation.

- Use `just` as the operator entrypoint and Bazel labels as the build graph.
- `dcxctl` is offline-only by default. Its sole exception is the explicit
  macOS-only `live-discovery` feature, which accepts one bounded private stdin
  envelope and can invoke only the review-gated Search carrier in
  `dcx-darwin-tty`. Live use still requires Legalab's exact attended WORD gate;
  no enumeration, arbitrary frame, generic write, or configuration command is
  allowed.
- Unknown identity, state, route, or value is an error. Never infer that an
  output is safe or muted.
- Tests use synthetic or explicitly sanitized fixtures. Never commit captures,
  device serials, credentials, proprietary software, or calibration files.
- Preserve source provenance and per-file licenses. Do not claim an observed
  protocol fact from a synthetic fixture.
- Use pull requests and run `just check` before claiming completion.
- Never add AI attribution or `Co-Authored-By` trailers.
