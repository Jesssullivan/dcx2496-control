#!/usr/bin/env bash
# Print the JBL protection plan and prove it decodes. Offline only: this
# script runs `dcxctl write plan` and `dcxctl write show`, which open no device.
#
#   scripts/jbl-protection-plan-dry-run.sh
#       Print every apply, then chain the plans on a synthetic 20a5b485 in
#       `cargo test` (frames, codes and final typed state).
#   scripts/jbl-protection-plan-dry-run.sh --snapshot PATH [--dcxctl PATH]
#       Also chain the plans on the private 20a5b485 capture and require every
#       digest, frame and plan digest in expected.json.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
plan_dir="$root/docs/plans/jbl-protection-20a5b485"
expected="$plan_dir/expected.json"
snapshot=""
dcxctl=""

while (($#)); do
    case "$1" in
        --snapshot) snapshot="${2:?--snapshot needs a path}"; shift 2 ;;
        --dcxctl) dcxctl="${2:?--dcxctl needs a path}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 64 ;;
    esac
done

echo "JBL protection plan, baseline $(jq -r .baseline_snapshot_digest "$expected")"
jq -r '.applies[] |
    "apply \(.apply)  \(.plan)\n" +
    (.writes | map("  O\(.output) \(.field): code \(.before_code) -> \(.after_code) \(.after | tostring)") | join("\n")) +
    "\n  frame    \(.apply_frame_hex)\n  rollback \(.rollback_frame_hex)\n" +
    "  remote-mode writes: apply \(.wire_budget.apply_remote_mode_writes), rollback \(.wire_budget.rollback_remote_mode_writes)\n" +
    "  expected digest after apply: \(.desired_snapshot_digest)"' "$expected"

echo
echo "Synthetic chain (cargo test):"
(cd "$root" && cargo test --locked --jobs 2 -p dcx-core --test jbl_protection_plan --quiet)

[[ -n "$snapshot" ]] || { echo "No --snapshot given: device digests not checked."; exit 0; }

if [[ -z "$dcxctl" ]]; then
    (cd "$root" && cargo build --locked --jobs 2 -p dcxctl --quiet)
    dcxctl="$(cd "$root" && cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/debug/dcxctl"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$snapshot" "$work/base-0.json"

want="$(jq -r .baseline_snapshot_digest "$expected")"
have="$(jq -r .snapshot_digest "$work/base-0.json")"
[[ "$have" == "$want" ]] || { echo "snapshot is $have, plan needs $want" >&2; exit 1; }
"$dcxctl" write show --snapshot "$work/base-0.json" >/dev/null

count="$(jq '.applies | length' "$expected")"
for ((i = 1; i <= count; i++)); do
    plan="$(jq -r ".applies[$((i - 1))].plan" "$expected")"
    "$dcxctl" write plan --snapshot "$work/base-$((i - 1)).json" --plan "$plan_dir/$plan" >"$work/tx-$i.json"
    jq -e --slurpfile e "$expected" --argjson n "$((i - 1))" '
        ($e[0].applies[$n]) as $w |
        .baseline_snapshot_digest == $w.baseline_snapshot_digest and
        .desired_snapshot_digest == $w.desired_snapshot_digest and
        .apply_frame_hex == $w.apply_frame_hex and
        .rollback_frame_hex == $w.rollback_frame_hex and
        .apply_plan.plan_digest == $w.apply_plan_digest and
        .rollback_plan.plan_digest == $w.rollback_plan_digest and
        ([.writes[] | {output, field, before_code, after_code, after}] == $w.writes) and
        .linked_effects == $w.linked_effects' "$work/tx-$i.json" >/dev/null ||
        { echo "apply $i differs from expected.json" >&2; exit 1; }
    jq '.apply_plan.desired' "$work/tx-$i.json" >"$work/base-$i.json"
    echo "apply $i OK  $(jq -r .desired_snapshot_digest "$work/tx-$i.json")"
done

"$dcxctl" write show --snapshot "$work/base-$count.json" >"$work/final.json"
jq -e --slurpfile e "$expected" '
    [.outputs[] | select(.output <= 2) | {
        key: (.output | tostring),
        value: {
            gain_code: .fields.gain.code,
            high_pass_type_code: .fields["high_pass.type"].code,
            high_pass_frequency_code: .fields["high_pass.frequency"].code,
            limiter_enabled_code: .fields["limiter.enabled"].code,
            limiter_threshold_code: .fields["limiter.threshold"].code,
            mute_code: .fields.mute.code
        }}] | from_entries == $e[0].final.outputs' "$work/final.json" >/dev/null ||
    { echo "final O1/O2 decode differs from expected.json" >&2; exit 1; }
jq -r '.outputs[] | select(.output <= 4) |
    "O\(.output): gain \(.fields.gain.gain_db) dB, high-pass \(.fields["high_pass.type"].filter) \(.fields["high_pass.frequency"].frequency_hz) Hz, limiter \(.fields["limiter.enabled"].enabled) at \(.fields["limiter.threshold"].threshold_db) dB, muted \(.fields.mute.muted)"' "$work/final.json"
echo "Device chain OK: final digest $(jq -r .final.snapshot_digest "$expected")"
