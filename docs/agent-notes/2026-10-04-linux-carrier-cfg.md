# Linux carrier compilation boundary

Authority: R-N13, `dec-native-studio-20261004`, and
`dec-legalab-reset-20261004`. Root reported Linux `just check` at source
`65daa0c` failing Clippy with 35 dead-code errors in private Darwin carrier
internals before tests ran.

Observed from source: the public runtime entry points and native backend
already require macOS with native compilation enabled. Their private state
machine, syscall trait, receipt builder, and bounded read helpers were still
compiled in the ordinary Linux library. Linux tests exercise those internals
with an injected fake backend.

Changes apply the existing native-or-test condition consistently to those
private internals and their imports:
`any(test, all(target_os = "macos", not(bazel_test_no_native)))`.
The opaque binding retains its path only where the state machine can use it;
its public constructor still validates the same path and produces the same
digest on every platform. Public offline bindings, errors, receipt types, and
operation-kind conversion remain available. All existing Linux fake tests
retain the complete private state machine through `cfg(test)`. The normal
Darwin product path evaluates the condition to true and is unchanged.

No new lint allowance, test skip, transport operation, or native syscall
change is introduced. Source whitespace inspection passed. Compilation,
Clippy, formatting, and tests remain unverified in this lane; root owns the
next exact-revision validation. This lane ran no builds, tests, Nix, host,
device, staging, or commit commands.

Root's reported Sting retry at `8777623` removed the private dead-code errors
and reached one Clippy `needless_pass_by_value` error in the offline binding
constructor. The follow-up explicitly drops the owned `PathBuf` after
validation and digesting only when native/test storage is absent. This keeps
the public owned-argument API and native/test path retention intact. The
follow-up has not been compiled or tested in this source lane.
