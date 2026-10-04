# Source frontend convergence

Authority: `dec-local-first-reapi-20261004`,
`dec-native-studio-20261004`, and R-N13. This lane owns source frontend changes
in the isolated studio sprint branch; root owns Git, execution and PR receipts.

Observed in source: the inherited unit-test frontend required GloriousFlywheel
enrollment, and hosted advisory invoked Rust compilation and Bazel tests.
The platform also carried a `gf.platform` property despite generic REAPI dispatch.

The frontend now selects bounded local Bazelisk by default and explicit REAPI
only with an operator executor and absolute external rc. It excludes ambient
rc sources, refuses startup options in the command position, suppresses rc
output, and forces the selected action/test strategies after caller arguments.
The anchored GNU-realpath Nix binding and Darwin SDK environment scrub remain.
No provider execution property, concrete endpoint or credential is committed.
Operator authentication and executor properties belong in the external rc.

Hosted advisory checks formatting, shell/workflow lint, schema syntax and
tracked nonempty lockfiles. Full Cargo checks remain an eligible-host lane.
Compiling Cargo frontend commands use `--locked --jobs 2`; test commands also
use two test threads. Exported Cargo job bounds cover nested frontend children.
The checked-in Rust toolchain and dependency locks are unchanged. Formatting
does not gain dependency-resolution flags. Apple project source already uses
the explicitly selected pinned Cargo/Rustc and `--locked` for its child build.

The no-build refusal regression has its own host-dependent Bazel label,
`//:bazel_frontdoor_rejections`, tagged no-remote/no-remote-cache/no-remote-exec.
It is included in `//:check` and the Just source check. Neither its tag filters
nor a cache hit establishes global REAPI eligibility or new remote execution.

Observed author-lane verification: Bash syntax, Shellcheck, Buildifier,
Actionlint, Just parsing/dry-runs and whitespace checks passed before the Cargo
bound follow-up. Buildifier, Just parsing and full-check/dcxctl dry-runs, and
whitespace checks also passed for the follow-up. Execution still requires root's
exact-revision eligible-host receipt. No tests, builds, Nix evaluation,
native compilation, staging, commits or host/device mutation ran in this lane.
Remote execution, signed artifacts, helper IPC, Logic and serial/device behavior
remain unverified by these source changes.
