#!/usr/bin/env bash
# Offline typed-write pipeline over synthetic snapshots: show -> dry-run plan
# -> exact frame bytes -> written-value verification, plus the reserved-output,
# routing and level refusals. Opens no device.
set -euo pipefail

dcxctl=$1
snapshot=$2

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

fail() {
  echo "$1" >&2
  exit 1
}

plan_doc() {
  printf '{"schema":"dcx.write-plan/v1","device_id":0,"changes":[%s]}\n' "$1" >"${work}/$2"
}

"${dcxctl}" write show --snapshot "${snapshot}" >"${work}/state.json"
jq -e '.schema == "dcx.typed-state/v1" and (.outputs | length) == 6' "${work}/state.json" >/dev/null

hpf='{"kind":"crossover","output":4,"side":"high_pass","filter":"lr24","frequency_hz":100}'
plan_doc "${hpf}" hpf.json

# The blank fixture has O5/O6 unmuted: every typed plan is refused.
if "${dcxctl}" write plan --snapshot "${snapshot}" --plan "${work}/hpf.json" \
  >/dev/null 2>"${work}/error.txt"; then
  fail "write plan accepted a baseline with O5/O6 unmuted"
fi
grep -q 'ReservedOutputUnmuted { output: 5' "${work}/error.txt" || fail "unexpected reserved-output refusal"

# Derive a synthetic baseline with O5/O6 muted through the routing lane.
"${dcxctl}" routing desired-profile --o5-mute on --o6-mute on --profile-id reserve \
  --revision synthetic-1 >"${work}/reserve.json"
"${dcxctl}" control diff --snapshot "${snapshot}" --profile "${work}/reserve.json" \
  | jq -e '.apply_plan.desired' >"${work}/muted.json"

"${dcxctl}" write plan --snapshot "${work}/muted.json" --plan "${work}/hpf.json" >"${work}/tx.json"
jq -e '.schema == "dcx.write-transaction/v1" and .dry_run == true' "${work}/tx.json" >/dev/null
frame=$(jq -r '.apply_frame_hex' "${work}/tx.json")
[[ ${frame} == "F0002032000E20020843004A08420008F7" ]] || fail "unexpected apply frame: ${frame}"
rollback=$(jq -r '.rollback_frame_hex' "${work}/tx.json")
[[ ${rollback} == "F0002032000E20020842000008430000F7" ]] || fail "unexpected rollback frame: ${rollback}"
jq -e '.wire_budget.apply_direct_frames == 1 and .wire_budget.apply_remote_mode_writes == 5' \
  "${work}/tx.json" >/dev/null

jq -e '.apply_plan' "${work}/tx.json" >"${work}/apply-plan.json"
jq -e '.apply_plan.desired' "${work}/tx.json" >"${work}/readback.json"
"${dcxctl}" write verify --plan "${work}/apply-plan.json" --readback "${work}/readback.json" \
  >"${work}/verify.json"
jq -e '.status == "consistent"' "${work}/verify.json" >/dev/null
if "${dcxctl}" write verify --plan "${work}/apply-plan.json" --readback "${work}/muted.json" \
  >/dev/null; then
  fail "write verify accepted the unchanged baseline as a readback"
fi

# Refusals: O6 target, EQ boost, source without consent, a +5 dB level rise.
refuse() {
  plan_doc "$1" refused.json
  if "${dcxctl}" write plan --snapshot "${work}/muted.json" --plan "${work}/refused.json" \
    >/dev/null 2>"${work}/error.txt"; then
    fail "write plan accepted: $1"
  fi
  grep -q "$2" "${work}/error.txt" || fail "unexpected refusal for $1: $(cat "${work}/error.txt")"
}
refuse '{"kind":"output_gain","output":6,"gain_db":-20}' 'ReservedOutput { change: 0, output: 6 }'
refuse '{"kind":"peq_band","output":1,"band":1,"filter":"bell","frequency_hz":1000,"gain_db":2,"q":1}' 'EqBoost'
refuse '{"kind":"output_source","output":3,"source":"b"}' 'RoutingNotAllowed'
refuse '{"kind":"output_gain","output":1,"gain_db":-10}' 'GainStep'
refuse '{"kind":"output_gain","output":1,"gain_db":-14,"bytes":"F0"}' 'unknown field'
