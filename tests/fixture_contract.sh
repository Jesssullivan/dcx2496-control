#!/usr/bin/env bash
set -euo pipefail

dcxctl=$1
protocol_fixture=$2
safe_profile=$3
desired_profile=$4
rew_fixture=$5
digest=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

"${dcxctl}" decode --file "${protocol_fixture}" >/dev/null
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
