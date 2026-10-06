# Host recall invalidates previous operation summaries

Authority: R-HOOK-CONVERGENCE-20261004 / R-N13,
dec-native-studio-20261004 and dec-local-first-reapi-20261004.
Product scope: TIN-5383 under TIN-4043; source base main
6b6efc6a1752534a3b0b99a5a792c3fa2fcf6881.

Observed source defect: restoring a Logic document into the same AU instance
replaced its model but preserved an earlier preview or rollback-success banner.
The desired state and snapshot labels could therefore describe another project
from the operation summary. Installed Logic behavior remains unverified.

An accepted host restoration now advances an instance-local generation under the
model lock. Presentation invalidates the old summary once per restoration,
including identical documents, legacy desired-only carriers and empty state.
Restored recovery gets an explicit recovery message. Ordinary refreshes and
fresh explicit results retain their summaries; a delayed host notification
cannot erase a result recorded after recall. Rejected replacement during active
recovery preserves the pinned transaction. The generation is absent from the
persisted v1 wire carrier. Restoration and presentation perform no helper IPC,
process, socket, serial or device operation.

Four added native fixture methods cover different and identical project recall,
legacy/empty/invalid-legacy carriers, recovery preservation and presentation
ordering. `just native-logic-recall` / `//apple:logic_recall` now selects 13
methods at the first checkpoint. A follow-up source counterexample retains the
same desired profile and snapshot after recall while a Preview reply is pending:
digest-only acceptance adds the old preview to the newly restored document.
A pending Snapshot reply can similarly clear its restored diff. Snapshot/Preview
now carry AU identity and request generation; acceptance checks generation under
the model lock and summaries retain request provenance. Superseded preparation
replies are discarded without retry. Mutation replies still reconcile pinned
recovery. Two additional matching-state fixtures bring the combined selection
to 15 methods. Actual native execution, focused unsigned AU/UI compilation, qualified
Linux checks/history scan and final independent review are required before this
source change merges; they are pending at this source checkpoint.

This does not establish current installed helper/AU identity, actual Logic
zero-write recall, complete scene control, serial apply/readback/rollback or
audio acceptance. Those remain separately admitted root-owned bench work.
