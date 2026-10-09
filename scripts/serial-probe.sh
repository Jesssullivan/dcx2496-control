#!/usr/bin/env bash
# dec-autonomous-muted-bench-20261007 / R-HOOK-CONVERGENCE-20261004 (R-N11/R-N13):
# One O4 PEQ named-device round trip: fresh rollback snapshot -> diff -> apply
# -> exact readback -> containment gate -> rollback -> readback == original.
#
# Bench harness only. Legalab owns readiness, the bench lock, and the muted
# downstream; run this under its lockf. The checks below only refuse; they
# never decide that a write is safe.
#
# Usage: DCXCTL=/path/to/live-control/dcxctl serial-probe.sh inactive|notch TAG
#   inactive  change only the first inactive O4 band's frequency code by one step
#   notch     one synthetic shallow notch from a ring-out list (planner path)
#
# Exits non-zero when the final readback differs from the original snapshot,
# when the apply readback is not verified or missing, or when any byte
# changed outside the plan's projected addresses and touched dump trailer
# (`dcxctl control verify-containment`). Rollback always runs first.
set -euo pipefail
mode=${1:?inactive|notch}
tag=${2:?tag}
D=${DCXCTL:?set DCXCTL to the unsandboxed live-control dcxctl}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
st="${DCX_PROBE_STATE:-$HOME/.local/state/legalab/dcx2496/probe}/$tag"
mkdir -p "$st"
chmod 700 "$st"
umask 077
shopt -s nullglob
ttys=(/dev/cu.usbserial-*)
shopt -u nullglob
[[ ${#ttys[@]} -eq 1 ]] || {
  echo "need exactly one FTDI callout" >&2
  exit 75
}
TTY=${ttys[0]}
[[ $({ lsof "$TTY" 2>/dev/null || true; } | tail -n +2 | wc -l | tr -d ' ') == 0 ]] || {
  echo "tty held" >&2
  exit 75
}
ts() { date -u +%TZ; }
echo "PROBE mode=$mode tag=$tag start=$(date -u +%FT%TZ) dcxctl_sha256=$(shasum -a 256 "$D" | cut -d' ' -f1)"
t0=$(date +%s)
"$D" control snapshot --tty "$TTY" --expected-device 0 >"$st/baseline.json"
base=$(jq -r .snapshot_digest "$st/baseline.json")
echo "BASELINE $(ts) secs=$(($(date +%s) - t0)) digest=$base"
python3 -I "$here/serial_probe_route.py" "$st/baseline.json" >"$st/route-baseline.json"
jq -c '{input_c_gain,output_muted,output_source,setup_mute_outs_raw}' "$st/route-baseline.json"
[[ $(jq -r .input_c_gain "$st/route-baseline.json") == line ]] || {
  echo "Input C not line; refusing write" >&2
  exit 76
}
if [[ $(jq -r '.output_muted.O5 and .output_muted.O6' "$st/route-baseline.json") != true ]]; then
  if [[ "${A4_O56_UNCHANGED_ONLY:-0}" == 1 ]]; then
    echo "WARN O5/O6 decode as unmuted at baseline (unverified DuinoDCX offsets). Proceeding: the containment gate below requires every byte outside the O4 PEQ projection, incl. O5/O6 mutes, to stay unchanged."
  else
    echo "O5/O6 not both muted; refusing write" >&2
    exit 76
  fi
fi
"$D" feedback inspect --snapshot "$st/baseline.json" --target-output 4 >"$st/o4-before.json"
echo "O4_BEFORE $(jq -c '{eq_enabled,eq_count}' "$st/o4-before.json")"
case "$mode" in
inactive)
  n=$(jq -r .eq_count "$st/o4-before.json")
  k=$((n + 1))
  ((k <= 9)) || {
    echo "no inactive O4 band" >&2
    exit 76
  }
  f=$(jq -r --argjson k "$k" '.bands[]|select(.band==$k)|.codes.frequency_code' "$st/o4-before.json")
  if ((f < 320)); then nf=$((f + 1)); else nf=$((f - 1)); fi
  param=$((0x13 + (k - 1) * 5))
  echo "PLAN inactive band=$k param=0x$(printf %02x "$param") frequency_code $f -> $nf"
  python3 -I "$here/serial_probe_v2.py" "$st/profile.json" o4-probe "inactive-b$k-f$nf" 4 \
    "[{\"channel\":8,\"parameter\":$param,\"value\":$nf}]"
  ;;
notch)
  printf 'frequency_hz,level_db\n2500,1.0\n' >"$st/ring-out-SYNTHETIC.txt"
  "$D" feedback import --frequency-list "$st/ring-out-SYNTHETIC.txt" --target-output 4 >"$st/measurement.json"
  "$D" feedback plan --measurement "$st/measurement.json" --snapshot "$st/baseline.json" \
    --max-notches 1 --max-cut-db -3 >"$st/plan.json"
  echo "PLAN notch $(jq -c '[.. | objects | select(has("band") and has("frequency_hz"))]' "$st/plan.json" | head -c 800)"
  "$D" feedback desired-profile --plan "$st/plan.json" --profile-id o4-synthetic \
    --revision synthetic-shallow-1 >"$st/profile.json"
  ;;
*)
  echo "bad mode" >&2
  exit 2
  ;;
esac
"$D" control diff --snapshot "$st/baseline.json" --profile "$st/profile.json" >"$st/diff.json"
echo "DIFF $(jq -c '{desired_profile_schema,changes}' "$st/diff.json")"
jq '.apply_plan' "$st/diff.json" >"$st/apply-plan.json"
jq '.rollback_plan' "$st/diff.json" >"$st/rollback-plan.json"
desired=$(jq -r .desired_snapshot_digest "$st/diff.json")

failures=()
t0=$(date +%s)
"$D" control apply --tty "$TTY" --expected-device 0 --plan "$st/apply-plan.json" >"$st/apply.json" ||
  echo "APPLY_EXIT=$?"
astat=$(jq -r .status "$st/apply.json" 2>/dev/null || echo error)
echo "APPLY $(ts) secs=$(($(date +%s) - t0)) status=$astat readback_matches_desired=$(jq -r .readback_matches_desired "$st/apply.json" 2>/dev/null) desired=$desired observed=$(jq -r '.readback.snapshot_digest // "null"' "$st/apply.json" 2>/dev/null)"
[[ "$astat" == verified ]] || failures+=("apply status $astat")
if [[ "$astat" != error ]] && jq -e '.readback != null' "$st/apply.json" >/dev/null; then
  jq '.readback' "$st/apply.json" >"$st/applied.json"
  "$D" feedback inspect --snapshot "$st/applied.json" --target-output 4 >"$st/o4-applied.json"
  echo "O4_APPLIED $(jq -c '{eq_enabled,eq_count,active:[.bands[]|select(.active)|{band,frequency_hz,gain_db,q}]}' "$st/o4-applied.json")"
  if "$D" control verify-containment --plan "$st/apply-plan.json" --readback "$st/applied.json" >"$st/containment.json"; then
    echo "CONTAINMENT contained projected=$(jq -c '[.projected_offsets[]|"\(.section):\(.offset)"]' "$st/containment.json")"
  else
    echo "CONTAINMENT FAILED $(jq -c '{status,uncontained_changes}' "$st/containment.json" 2>/dev/null || echo unreadable)" >&2
    failures+=("bytes changed outside the projected addresses")
  fi
else
  failures+=("no apply readback; containment unproven")
fi

t0=$(date +%s)
"$D" control rollback --tty "$TTY" --expected-device 0 --plan "$st/rollback-plan.json" >"$st/rollback.json" ||
  echo "ROLLBACK_EXIT=$?"
echo "ROLLBACK $(ts) secs=$(($(date +%s) - t0)) status=$(jq -r .status "$st/rollback.json" 2>/dev/null) equals_baseline=$(jq -r .equals_baseline "$st/rollback.json" 2>/dev/null)"
t0=$(date +%s)
"$D" control readback --tty "$TTY" --expected-device 0 >"$st/final.json"
final=$(jq -r .snapshot_digest "$st/final.json")
echo "FINAL_READBACK $(ts) secs=$(($(date +%s) - t0)) digest=$final equals_original=$([[ "$final" == "$base" ]] && echo true || echo false)"
[[ "$final" == "$base" ]] || failures+=("final readback differs from the original snapshot")
python3 -I "$here/serial_probe_route.py" "$st/final.json" | jq -c '{input_c_gain,output_muted,output_source}'
for f in applied final; do
  [[ -f "$st/$f.json" ]] || continue
  python3 -I "$here/serial_probe_route.py" "$st/$f.json" >"$st/route-$f.json"
  echo "O56_CHECK $f $(jq -c '[.output_mute_raw.O5,.output_mute_raw.O6,.input_c_gain]' "$st/route-$f.json") baseline $(jq -c '[.output_mute_raw.O5,.output_mute_raw.O6,.input_c_gain]' "$st/route-baseline.json")"
done
echo "PROBE_END mode=$mode end=$(date -u +%FT%TZ) failures=${#failures[@]}"
if ((${#failures[@]})); then
  printf 'PROBE_FAIL %s\n' "${failures[@]}" >&2
  exit 1
fi
