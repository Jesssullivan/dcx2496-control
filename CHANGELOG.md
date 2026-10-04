# Changelog

## Unreleased

- Report Identity Search qualification counts from its exact Rust error format.
- Preserve snapshot failure stage and explicit cleanup failure through one
  transaction `Capture(...)` envelope, retaining bounded inspection and unknown
  classification for unsupported formats.
- Distinguish bundled child launch failures and output-limit failures from
  malformed responses without automatic retry or recovery-state completion.
