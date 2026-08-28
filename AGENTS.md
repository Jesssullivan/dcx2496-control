# DCX2496 Control Agent Contract

This repository owns the typed, deterministic DCX2496 protocol and profile
runtime. Legalab owns the cross-repository studio ontology and PZM activation.

- Use `just` as the operator entrypoint and Bazel labels as the build graph.
- `dcxctl` is offline-only by default. Its sole exception is the explicit
  macOS-only `live-discovery` feature, whose `discovery live-search` command
  accepts one named callout path, sends only the typed Search query, and runs
  exactly nine repeats at the MVP 38400 binding. It never enumerates ports or
  exposes arbitrary frames, generic writes, or configuration commands.
- Legalab owns physical/operator readiness before invoking live Search. Do not
  duplicate Legalab decisions, evidence schemas, or authorization ceremonies in
  this device repository.
- Unknown identity, state, route, or value is an error. Never infer that an
  output is safe or muted.
- Tests use synthetic or explicitly sanitized fixtures. Never commit captures,
  device serials, credentials, proprietary software, or calibration files.
- Preserve source provenance and per-file licenses. Do not claim an observed
  protocol fact from a synthetic fixture.
- Use pull requests and run `just check` before claiming completion.
- Never add AI attribution or `Co-Authored-By` trailers.
