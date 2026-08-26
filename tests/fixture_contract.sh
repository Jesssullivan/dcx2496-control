#!/usr/bin/env bash
set -euo pipefail

dcxctl=$1
protocol_fixture=$2
search_fixture=$3
safe_profile=$4
desired_profile=$5
rew_fixture=$6
digest=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

"${dcxctl}" decode --file "${protocol_fixture}" >/dev/null
"${dcxctl}" decode --file "${search_fixture}" | grep -q '"kind": "search_response"'
plan_output=$("${dcxctl}" discovery plan --expected-device 0)
grep -q '"kind": "primary"' <<<"${plan_output}"
grep -q '"baud": 115200' <<<"${plan_output}"
grep -q '"kind": "single_fallback"' <<<"${plan_output}"
grep -q '"baud": 38400' <<<"${plan_output}"
grep -q '"query_hex": "F0002032200E40F7"' <<<"${plan_output}"
grep -q '"transport_opened": false' <<<"${plan_output}"
if [[ $(grep -c '"baud":' <<<"${plan_output}") -ne 2 ]]; then
  echo "discovery plan exposed an unexpected attempt count" >&2
  exit 1
fi
"${dcxctl}" discovery validate-response "${search_fixture}" --expected-device 0 >/dev/null
discovery_help=$("${dcxctl}" discovery --help)
if grep -Eq '(^|[[:space:]])(prepare|live|repeat)([[:space:]]|$)' <<<"${discovery_help}"; then
  echo "default dcxctl unexpectedly exposes live discovery" >&2
  exit 1
fi
"${dcxctl}" profile validate "${safe_profile}" >/dev/null
"${dcxctl}" profile diff "${safe_profile}" "${desired_profile}" >/dev/null
"${dcxctl}" rew import "${rew_fixture}" --target-output 3 >/dev/null

if apply_output=$("${dcxctl}" profile apply-ready "${safe_profile}" --profile-digest "${digest}" 2>&1); then
  echo "unverified sink models crossed the apply-readiness gate" >&2
  exit 1
fi
if [[ ${apply_output} != *"UnverifiedSink(3)"* ]]; then
  echo "apply-readiness fixture failed for an unexpected reason" >&2
  exit 1
fi

if boost_output=$("${dcxctl}" rew import "${rew_fixture}" --target-output 3 --allow-boost 2>&1); then
  echo "removed boost option remains reachable" >&2
  exit 1
fi
if [[ ${boost_output} != *"unexpected argument '--allow-boost'"* ]]; then
  echo "removed boost option failed for an unexpected reason" >&2
  exit 1
fi

: "${TEST_TMPDIR:?Bazel must provide a per-test scratch directory}"
oversized_rew="${TEST_TMPDIR}/oversized.rew"
dd if=/dev/zero of="${oversized_rew}" bs=65537 count=1 2>/dev/null
if "${dcxctl}" rew import "${oversized_rew}" --target-output 3 >/dev/null 2>&1; then
  echo "oversized REW input crossed the CLI file-read bound" >&2
  exit 1
fi
