# Request-bound helper replies

Authority: `R-HOOK-CONVERGENCE-20261004`, R-N13,
`dec-native-studio-20261004` and `dec-local-first-reapi-20261004`.
Owning product ticket: TIN-5383 under TIN-4043.
Source base: `0db521cfabd233ed520a994de58791decbfa6d6e`.

Observed source: the socket client correlated reply request IDs, while standalone
reply validation checked internal shape and receipt operation. The client did
not bind reply operation, target, transaction, baseline or desired digest to its
request. Readback's `matchesDesired` assertion was not checked against the
requested digest before the controller displayed its result. The helper's
current implementation computes that assertion correctly; this is a boundary
hardening gap, not an observed device or production failure.

The client now validates each complete reply against the exact request. Success
and error replies require matching request ID and operation. Identity replies
must name the requested address; snapshot and mutation captures must name the
requested target. Diff, Apply and Rollback bind their carried digests and
transaction to the request. Readback equality must match the requested desired
digest in both directions. Missing Apply/Rollback capture remains accepted as
explicit uncertainty with the existing false equality and recovery flags.

Seven pure native methods use generated operation pairs, transaction/digest
variants, target/address changes, contradictory equality assertions and missing
captures. Inputs are synthetic summaries serialized through the actual bridge
codec. They invoke no socket, helper, CoreMIDI endpoint, child, serial or device.
`just native-response-binding` selects Darwin-only manual/local/no-remote Bazel
label `//apple:response_binding` with one Swift job and an owned scratch root.

Native execution, qualified Linux source/history checks and independent final
review remain pending. The separate first-Apply revision is frozen for its
earlier test allocation. Full scene semantics, actual Logic zero-serial recall,
named-device complete readback/revert and automatic hardware recovery remain
unverified; this change creates no hardware authorization or mutation path.
