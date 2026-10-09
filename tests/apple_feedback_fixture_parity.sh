#!/usr/bin/env bash
# Rust/Swift parity for the bridge v2 feedback-notch fixtures. The Swift helper
# and AU tests read dcxctl outputs checked in under apple/Tests; regenerate
# them from the synthetic ring-out and blank snapshot with the dcxctl under
# test and require the exact bytes (the large control diff is compared in jq's
# canonical compact form). Offline; opens no device.
set -euo pipefail

if [[ $# != 4 ]]; then
  printf 'usage: apple_feedback_fixture_parity.sh DCXCTL RING_OUT SNAPSHOT FIXTURE_DIR|FIXTURE_FILE\n' >&2
  exit 64
fi
dcxctl=$1
ring_out=$2
snapshot=$3
fixtures=$4
# Bazel names one declared fixture file; the directory holds the whole set.
if [[ -f "${fixtures}" ]]; then
  fixtures=$(dirname "${fixtures}")
fi

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

cmp "${ring_out}" "${fixtures}/SYNTHETIC-ring-out.txt"
cmp "${snapshot}" "${fixtures}/SYNTHETIC-blank-snapshot.json"

"${dcxctl}" feedback import --frequency-list "${ring_out}" --target-output 4 >"${work}/measurement.json"
cmp "${work}/measurement.json" "${fixtures}/SYNTHETIC-o4-measurement.json"

"${dcxctl}" feedback plan --measurement "${work}/measurement.json" --snapshot "${snapshot}" >"${work}/plan.json"
cmp "${work}/plan.json" "${fixtures}/SYNTHETIC-o4-notch-plan.json"

"${dcxctl}" feedback desired-profile --plan "${work}/plan.json" \
  --profile-id o4-feedback --revision synthetic-1 >"${work}/profile.json"
cmp "${work}/profile.json" "${fixtures}/SYNTHETIC-o4-desired-profile-v2.json"

"${dcxctl}" control diff --snapshot "${snapshot}" --profile "${work}/profile.json" >"${work}/diff.json"
jq -cS . "${work}/diff.json" >"${work}/diff.canonical.json"
cmp "${work}/diff.canonical.json" "${fixtures}/SYNTHETIC-o4-diff-v2.json"
