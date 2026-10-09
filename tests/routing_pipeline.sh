#!/usr/bin/env bash
# Offline closed MVP routing pipeline over a synthetic snapshot
# (dec-autonomous-muted-bench-20261007): inspect -> desired routing profile ->
# control diff, the fixed apply/rollback order, and the refusals. Opens no
# device.
set -euo pipefail

dcxctl=$1
snapshot=$2

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
grep -q '"field": "mute"' "${work}/diff.json"

# Apply order is mutes, input sum, O4 source, O3 source; rollback reverses it.
actions_of() {
  tr -d ' \n' <"${work}/diff.json" | sed -e "$1" |
    grep -o '"channel":[0-9]*,"parameter":[0-9]*' |
    sed -e 's/"channel"://' -e 's/"parameter"://' | tr '\n' ' '
}
apply=$(actions_of 's/.*"apply_plan":.*"command":{"actions":\[\([^]]*\)\].*"rollback_plan".*/\1/')
[[ ${apply} == "9,3 10,3 0,2 8,65 7,65 " ]] || fail "unexpected apply order: ${apply}"
rollback=$(actions_of 's/.*"rollback_plan":.*"command":{"actions":\[\([^]]*\)\].*/\1/')
[[ ${rollback} == "7,65 8,65 0,2 10,3 9,3 " ]] || fail "unexpected rollback order: ${rollback}"

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
