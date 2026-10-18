#!/usr/bin/env bash
# Offline closed MVP routing pipeline over synthetic snapshots
# (dec-autonomous-muted-bench-20261007): inspect -> desired routing profile ->
# control diff, the fixed apply/rollback order, and the refusals. Opens no
# device. The blank snapshot has O5/O6 unmuted, so no routing plan can be
# staged on it (the exact rollback of muting them would unmute them); the
# reserve-muted snapshot is the blank one with only O5/O6 muted.
set -euo pipefail

dcxctl=$1
blank=$2
snapshot=$3

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

fail() {
  echo "$1" >&2
  exit 1
}

"${dcxctl}" routing inspect --snapshot "${snapshot}" >"${work}/state.json"
grep -q '"schema_version": "dcx.routing-state/v1"' "${work}/state.json"
grep -q '"input_sum_code": 0' "${work}/state.json"

"${dcxctl}" routing desired-profile --mvp --profile-id mvp-routing \
  --revision synthetic-1 >"${work}/profile.json"
grep -q '"schemaVersion": "dcx.desired-routing/v1"' "${work}/profile.json"

"${dcxctl}" control diff --snapshot "${snapshot}" --profile "${work}/profile.json" >"${work}/diff.json"
grep -q '"desired_profile_schema": "dcx.desired-routing/v1"' "${work}/diff.json"
grep -q '"field": "setup.input_sum"' "${work}/diff.json"
grep -q '"field": "source"' "${work}/diff.json"
# O5/O6 already read muted, so their mute writes are no-op values of 1.
if grep -q '"field": "mute"' "${work}/diff.json"; then
  fail "reserve-muted baseline reported a mute change"
fi

# No carrier on any lane ever unmutes O5/O6: muting them from live cannot be
# staged, because the exact rollback would unmute them.
if "${dcxctl}" control diff --snapshot "${blank}" --profile "${work}/profile.json" \
  >/dev/null 2>"${work}/error.txt"; then
  fail "control diff staged a routing plan whose rollback unmutes O5/O6"
fi
grep -q 'ReservedUnmute' "${work}/error.txt" || fail "unexpected reserved refusal: $(cat "${work}/error.txt")"
for reserved in --o5-mute --o6-mute; do
  if "${dcxctl}" routing desired-profile "${reserved}" off --profile-id x --revision y \
    >/dev/null 2>&1; then
    fail "routing desired-profile accepted ${reserved} off"
  fi
done

# Apply order is mutes, input sum, O4 source, O3 source; rollback reverses it.
# control diff sends changed fields only, so the already-muted O5/O6 drop out.
actions_of() {
  tr -d ' \n' <"${work}/diff.json" | sed -e "$1" |
    grep -o '"channel":[0-9]*,"parameter":[0-9]*' |
    sed -e 's/"channel"://' -e 's/"parameter"://' | tr '\n' ' '
}
apply=$(actions_of 's/.*"apply_plan":.*"command":{"actions":\[\([^]]*\)\].*"rollback_plan".*/\1/')
[[ ${apply} == "0,2 8,65 7,65 " ]] || fail "unexpected apply order: ${apply}"
rollback=$(actions_of 's/.*"rollback_plan":.*"command":{"actions":\[\([^]]*\)\].*/\1/')
[[ ${rollback} == "7,65 8,65 0,2 " ]] || fail "unexpected rollback order: ${rollback}"

# A mute still goes first: the O4 mute (live in the fixture) ahead of the sum.
"${dcxctl}" routing desired-profile --o4-mute on --input-sum a-plus-b --profile-id o4-sum \
  --revision synthetic-1 >"${work}/o4-sum.json"
"${dcxctl}" control diff --snapshot "${snapshot}" --profile "${work}/o4-sum.json" >"${work}/diff.json"
apply=$(actions_of 's/.*"apply_plan":.*"command":{"actions":\[\([^]]*\)\].*"rollback_plan".*/\1/')
[[ ${apply} == "8,3 0,2 " ]] || fail "unexpected O4 mute apply order: ${apply}"

# A single-field probe profile carries exactly one action.
"${dcxctl}" routing desired-profile --o5-mute on --profile-id probe-o5 \
  --revision synthetic-1 >"${work}/probe.json"
[[ $(grep -c '"channel":' "${work}/probe.json") -eq 1 ]] || fail "probe profile is not one action"

# An empty target set is refused.
if "${dcxctl}" routing desired-profile --profile-id x --revision y >/dev/null 2>&1; then
  fail "routing accepted an empty target set"
fi

# Hand-edited profiles outside the ruling are refused by scope validation in
# control diff: Input C gain (setup 0x04), crossover, O1 source, Mute Outs
# (setup 0x15), and input sum A.
for edit in 's/"parameter": 2,/"parameter": 4,/' \
  's/"parameter": 65,/"parameter": 66,/' \
  's/"channel": 8,/"channel": 5,/' \
  's/"parameter": 2,/"parameter": 21,/' \
  's/"value": 4/"value": 1/'; do
  sed -e "${edit}" "${work}/profile.json" >"${work}/edited.json"
  if cmp -s "${work}/profile.json" "${work}/edited.json"; then
    fail "edit did not change the profile: ${edit}"
  fi
  if "${dcxctl}" control diff --snapshot "${snapshot}" --profile "${work}/edited.json" \
    >/dev/null 2>"${work}/error.txt"; then
    fail "control diff accepted an out-of-scope routing profile: ${edit}"
  fi
  grep -q 'UnsupportedDocument' "${work}/error.txt" || fail "unexpected refusal for ${edit}"
done
