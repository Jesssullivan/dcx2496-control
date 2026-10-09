#!/usr/bin/env bash
# Offline static feedback-suppression pipeline over synthetic fixtures:
# import -> inspect -> plan -> desired profile v2 -> control diff ->
# verify-containment, plus the refusals. Opens no device.
set -euo pipefail

dcxctl=$1
ring_out=$2
rew_notches=$3
snapshot=$4

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

"${dcxctl}" feedback import --frequency-list "${ring_out}" --target-output 4 >"${work}/measurement.json"
grep -q '"schema_version": "dcx.feedback-measurement/v1"' "${work}/measurement.json"
"${dcxctl}" feedback import --rew "${rew_notches}" --target-output 4 | grep -q '"source": "rew_generic_eq"'

inspect=$("${dcxctl}" feedback inspect --snapshot "${snapshot}" --target-output 4)
grep -q '"eq_count": 0' <<<"${inspect}"
grep -q '"evidence_class": "transcribed_layout_unverified_on_named_device"' <<<"${inspect}"

"${dcxctl}" feedback plan --measurement "${work}/measurement.json" --snapshot "${snapshot}" >"${work}/plan.json"
grep -q '"schema_version": "dcx.notch-plan/v1"' "${work}/plan.json"
if [[ $(grep -c '"band":' "${work}/plan.json") -ne 3 ]]; then
  echo "synthetic ring-out did not plan exactly three notches" >&2
  exit 1
fi

"${dcxctl}" feedback desired-profile --plan "${work}/plan.json" \
  --profile-id o4-feedback --revision synthetic-1 >"${work}/profile.json"
grep -q '"schemaVersion": "dcx.desired-profile/v2"' "${work}/profile.json"

"${dcxctl}" control diff --snapshot "${snapshot}" --profile "${work}/profile.json" >"${work}/diff.json"
grep -q '"desired_profile_schema": "dcx.desired-profile/v2"' "${work}/diff.json"
grep -q '"field": "eq_count"' "${work}/diff.json"
grep -q '"field": "band1.gain"' "${work}/diff.json"

# Extract one pretty-printed JSON object member by key and exact indentation.
member() {
  awk -v first="$2\"$1\": {" -v last="$2}" '
    !found && $0 == first { found = 1; print "{"; next }
    found && ($0 == last || $0 == last ",") { print "}"; exit }
    found { print }
  ' "$3"
}
member apply_plan "  " "${work}/diff.json" >"${work}/apply-plan.json"
member desired "    " "${work}/apply-plan.json" >"${work}/desired.json"

# The plan's own desired readback and an unchanged readback are contained.
"${dcxctl}" control verify-containment --plan "${work}/apply-plan.json" \
  --readback "${work}/desired.json" >"${work}/contained.json"
grep -q '"status": "contained"' "${work}/contained.json"
grep -q '"readback_matches_desired": true' "${work}/contained.json"
"${dcxctl}" control verify-containment --plan "${work}/apply-plan.json" \
  --readback "${snapshot}" | grep -q '"status": "contained"'

# A one-notch plan does not contain the three-notch readback: exit non-zero.
"${dcxctl}" feedback plan --measurement "${work}/measurement.json" --snapshot "${snapshot}" \
  --max-notches 1 >"${work}/plan1.json"
"${dcxctl}" feedback desired-profile --plan "${work}/plan1.json" \
  --profile-id o4-feedback --revision synthetic-one >"${work}/profile1.json"
"${dcxctl}" control diff --snapshot "${snapshot}" --profile "${work}/profile1.json" >"${work}/diff1.json"
member apply_plan "  " "${work}/diff1.json" >"${work}/apply-plan1.json"
if "${dcxctl}" control verify-containment --plan "${work}/apply-plan1.json" \
  --readback "${work}/desired.json" >"${work}/uncontained.json"; then
  echo "containment gate accepted bytes outside the projected addresses" >&2
  exit 1
fi
grep -q '"status": "uncontained"' "${work}/uncontained.json"
grep -q '"section": "dump1"' "${work}/uncontained.json"

# O4 is the only feedback target.
"${dcxctl}" feedback import --frequency-list "${ring_out}" --target-output 1 >"${work}/o1.json"
if "${dcxctl}" feedback plan --measurement "${work}/o1.json" --snapshot "${snapshot}" >/dev/null 2>&1; then
  echo "feedback planner accepted a non-O4 output" >&2
  exit 1
fi
# The policy floor stays inside the device limit.
if "${dcxctl}" feedback plan --measurement "${work}/measurement.json" --snapshot "${snapshot}" \
  --max-cut-db -20 >/dev/null 2>&1; then
  echo "feedback planner accepted a cut beyond -15 dB" >&2
  exit 1
fi
