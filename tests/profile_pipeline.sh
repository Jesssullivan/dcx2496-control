#!/usr/bin/env bash
# Offline output-profile pipeline: transcribe a Legalab-style
# DcxOutputProfileV1 from `write show` of a synthetic snapshot, verify it,
# plan a chain to a changed profile, and check the refusals. Opens no device.
set -euo pipefail

dcxctl=$1
muted=$2
legalab_profile=$3

work=$(mktemp -d)
trap 'rm -rf "${work}"' EXIT

fail() {
  echo "$1" >&2
  exit 1
}

# The Legalab fixture parses; against the blank synthetic O4 its verify-only
# fields (source, limiter threshold) differ, so verify drifts and plan refuses.
if "${dcxctl}" profile verify --profile "${legalab_profile}" --snapshot "${muted}" >"${work}/drift.json"; then
  fail "profile verify passed a drifting snapshot"
fi
jq -e '.schema == "dcx.profile-verification/v1" and .status == "drift"' "${work}/drift.json" >/dev/null
if "${dcxctl}" profile plan --profile "${legalab_profile}" --snapshot "${muted}" \
  >/dev/null 2>"${work}/error.txt"; then
  fail "profile plan accepted verify-only drift"
fi
grep -q 'VerifyOnlyDrift' "${work}/error.txt" || fail "unexpected verify-only refusal"

# Transcribe O4 of the synthetic snapshot as Legalab does from write show.
"${dcxctl}" write show --snapshot "${muted}" >"${work}/show.json"
jq '
  def milli: (. * 1000 | round);
  def tenths: (. * 10 | round);
  def side($name): {
    type: .fields[$name + ".type"].filter,
    frequencyMilliHz: (.fields[$name + ".frequency"].frequency_hz | milli),
    code: .fields[$name + ".frequency"].code
  };
  .snapshot_digest as $digest
  | .outputs[3] as $o4
  | ($o4.fields.eq_count.code) as $count
  | {
      schemaVersion: "legalab.dcx-output-profile/v1",
      contractType: "DcxOutputProfileV1",
      id: ("dcx-o4-synthetic-" + $digest[7:15]),
      title: "SYNTHETIC",
      purpose: "SYNTHETIC",
      output: 4,
      sink: "alto-pa",
      capture: {
        typedStateSchema: "dcx.typed-state/v1",
        snapshotDigest: $digest,
        evidenceClass: "transcribed_layout_unverified_on_named_device",
        provenance: "observed-readback",
        observedAt: "2026-10-21T00:00Z",
        tool: "SYNTHETIC",
        laneRef: "SYNTHETIC",
        laneFileSha256: ("0" * 64)
      },
      state: {
        muted: $o4.fields.mute.muted,
        source: $o4.fields.source.source,
        gainDbTenths: ($o4.fields.gain.gain_db | tenths),
        polarityInverted: $o4.fields.polarity.inverted,
        highPass: ($o4 | side("high_pass")),
        lowPass: ($o4 | side("low_pass")),
        delay: {
          enabled: $o4.fields.delay_enabled.enabled,
          shortMm: $o4.fields.short_delay.short_mm,
          longMm: $o4.fields.long_delay.long_mm
        },
        limiter: {
          enabled: $o4.fields["limiter.enabled"].enabled,
          thresholdDbTenths: ($o4.fields["limiter.threshold"].threshold_db | tenths)
        },
        peq: {
          enabled: $o4.fields.eq_enabled.enabled,
          count: $count,
          bands: [$o4.fields.peq[:$count][] | {
            band, kind,
            frequencyMilliHz: (.frequency_hz | milli),
            gainDbTenths: (.gain_db | tenths),
            qThousandths: (.q | milli),
            codes: {
              frequency: .codes.frequency_code, gain: .codes.gain_code, q: .codes.q_code,
              kind: .codes.kind_code, slope: .codes.slope_code
            }
          }]
        }
      },
      limitations: "SYNTHETIC"
    }' "${work}/show.json" >"${work}/same.json"

"${dcxctl}" profile verify --profile "${work}/same.json" --snapshot "${muted}" >"${work}/match.json"
jq -e '.status == "match" and (.drift | length) == 0' "${work}/match.json" >/dev/null
"${dcxctl}" profile plan --profile "${work}/same.json" --snapshot "${muted}" >"${work}/noop.json"
jq -e '.status == "already_matching" and (.steps | length) == 0' "${work}/noop.json" >/dev/null

# One new -6 dB bell at 654 Hz, Q 6.31, switched on: one free transaction.
jq '.state.peq = {enabled: true, count: 1, bands: [{
      band: 1, kind: "bell", frequencyMilliHz: 654014, gainDbTenths: -60, qThousandths: 6310,
      codes: {frequency: 161, gain: 90, q: 36, kind: 1, slope: 1}}]}' \
  "${work}/same.json" >"${work}/bell.json"
"${dcxctl}" profile plan --profile "${work}/bell.json" --snapshot "${muted}" >"${work}/chain.json"
jq -e '
  .schema == "dcx.profile-chain/v1" and .dry_run and .status == "planned"
  and (.chain_id | startswith("sha256/"))
  and .budget.transactions == 1 and .budget.apply_remote_mode_writes_per_transaction == 5
  and .budget.rollback_remote_mode_writes_per_transaction == 3
  and .final_verification.status == "match"
  and (.steps[0].write_plan.schema == "dcx.write-plan/v1")
  and (.steps[0].transaction.writes | map(.parameter) | last) == 6
' "${work}/chain.json" >/dev/null || fail "unexpected chain"
# The chain is deterministic.
"${dcxctl}" profile plan --profile "${work}/bell.json" --snapshot "${muted}" >"${work}/again.json"
[[ $(jq -r .chain_id "${work}/chain.json") == $(jq -r .chain_id "${work}/again.json") ]] ||
  fail "the chain id is not deterministic"

# Nine bands and a boost are refused offline.
jq '.state.peq.count = 9' "${work}/bell.json" >"${work}/nine.json"
if "${dcxctl}" profile plan --profile "${work}/nine.json" --snapshot "${muted}" >/dev/null 2>"${work}/error.txt"; then
  fail "a nine-band profile was accepted"
fi
grep -q 'DeviceBandCount(9)' "${work}/error.txt" || fail "unexpected nine-band refusal"
jq '.state.peq.bands[0].codes.gain = 160 | .state.peq.bands[0].gainDbTenths = 10' \
  "${work}/bell.json" >"${work}/boost.json"
if "${dcxctl}" profile plan --profile "${work}/boost.json" --snapshot "${muted}" >/dev/null 2>"${work}/error.txt"; then
  fail "a boost profile was accepted"
fi
grep -q 'EqBoost { band: 1' "${work}/error.txt" || fail "unexpected boost refusal"

# The default build exposes no live chain apply.
profile_help=$("${dcxctl}" profile --help)
grep -Eq '^  plan( |$)' <<<"${profile_help}" || fail "profile plan is not listed"
if grep -Eq '^  apply( |$)' <<<"${profile_help}"; then
  fail "default dcxctl exposes profile apply"
fi
