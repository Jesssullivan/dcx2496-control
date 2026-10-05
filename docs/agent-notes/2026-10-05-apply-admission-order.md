# Explicit Apply admission ordering

Authority: `R-HOOK-CONVERGENCE-20261004`, R-N13,
`dec-native-studio-20261004`, and `dec-local-first-reapi-20261004`.
Owning product ticket: TIN-5383 under TIN-4043.

Observed source defect at `4d656cc27d8a7c8d531fa6144bc880df46da3309`:
the AU controller called `beginApplyAttempt` before `send(.apply)`. That
transition retained the transaction and rollback baseline, activating recovery.
The subsequent availability check rejected Apply because recovery was active,
then unwound the locally rejected attempt. The first ordinary Apply therefore
could not reach the helper. This is a source sequencing conclusion; no device
failure is asserted.

The controller now checks availability, the one-request bound, and request/client
preparation before its typed Apply admission. Admission rechecks authorization,
validates the exact current target, desired profile, baseline and diff, then
retains recovery immediately before dispatch. A second Apply remains blocked.
Local preflight failures report their result without invoking the dispatched
request's helper-rejection callback or clearing earlier recovery authority.

The native fixture family covers one explicit dispatch with recovery already
retained, denied authority, generated mismatched valid plan digests, wrong
target, typed pre-admission rejection and explicit retry, and lost-response
recovery across AU document restoration. Saved-state restoration performs no
automatic dispatch. The fixtures use synthetic summaries and an injected
transport closure; they do not establish observed DCX byte mappings.

Reproducible native front door: `just native-logic-recall`, Bazel label
`//apple:logic_recall`. It is Darwin-only, manual/local/no-remote, uses one Swift
job and an owned scratch directory. The SwiftPM graph excludes the controller,
so a separately qualified unsigned Apple bundle build must also compile the UI
integration before source acceptance. Shell front-door execution must be
labelled distinctly if the owning native lane cannot invoke Bazel.

Observed local source checks: `git diff --check`, `bash -n`, ShellCheck,
Buildifier check and Just parsing passed. Native fixtures, unsigned bundle
compilation, qualified Linux source/history checks and independent final review
remain pending at this source checkpoint.

This change does not extend the O1/PEQ9 or fixture-derived O4 mute contract. Full
PA/JBL scene control, Input C safety readback, installed helper/AU/Logic exchange,
zero serial writes during actual Logic recall, named-device apply/readback and
rollback, automatic hardware recovery and resumed audio remain unverified.
No serial, hardware, installation, driver, clock or audio operation was run.
