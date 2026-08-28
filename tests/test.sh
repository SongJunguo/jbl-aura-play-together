#!/usr/bin/env bash
set -euo pipefail

test_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "${test_dir}/.." && pwd)"

export JBL_AURA_CONFIG="${repo_dir}/config/devices.env.example"
export JBL_CONNECT_DELAY=0
export JBL_STEP_DELAY=0
export JBL_GATT_TIMEOUT=3
export AURA_GATT_TIMEOUT=3
export AURA_GATT_RETRIES=1

# shellcheck source=../bin/jbl-aura-link
source "${repo_dir}/bin/jbl-aura-link"

assert_equal() {
  local expected="$1" actual="$2" label="$3"
  if [[ "${expected}" != "${actual}" ]]; then
    printf 'FAIL %s\nexpected: %s\nactual:   %s\n' \
      "${label}" "${expected}" "${actual}" >&2
    exit 1
  fi
  printf 'PASS %s\n' "${label}"
}

assert_fails() {
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    printf 'FAIL %s: unexpectedly succeeded\n' "${label}" >&2
    exit 1
  fi
  printf 'PASS %s\n' "${label}"
}

assert_equal '504c011f0000' "$(build_pl_frame 7937)" 'ENTER frame'
assert_equal '504c021f0000' "$(build_pl_frame 7938)" 'EXIT frame'
assert_equal '504c151f0c007b22616374696f6e223a327d' \
  "$(build_pl_frame 7957 '{"action":2}')" 'STOP frame'

expected_payload='{"action":1,"broadcast":{"address":"02:00:00:00:00:01","name":"JBL Authentics 300","need_access_code":false,"status":2,"subgroup":[{"active_status":1,"index":0,"is_support":true,"quality":0}]}}'
assert_equal "${expected_payload}" "$(build_start_payload)" 'illustrative start JSON'

expected_frame='504c151fc1007b22616374696f6e223a312c2262726f616463617374223a7b2261646472657373223a2230323a30303a30303a30303a30303a3031222c226e616d65223a224a424c2041757468656e7469637320333030222c226e6565645f6163636573735f636f6465223a66616c73652c22737461747573223a322c2273756267726f7570223a5b7b226163746976655f737461747573223a312c22696e646578223a302c2269735f737570706f7274223a747275652c227175616c697479223a307d5d7d7d'
assert_equal "${expected_frame}" "$(build_pl_frame 7957 "${expected_payload}")" \
  'illustrative start golden frame'

utf8_payload='{"name":"测试"}'
utf8_frame="$(build_pl_frame 7957 "${utf8_payload}")"
assert_equal '1100' "${utf8_frame:8:4}" 'UTF-8 byte length is little-endian 17'

assert_equal 'RECEIVER(2)' \
  "$(decode_role '00 00 00 00 00 00 00 00 00 00 00 00 00 00 02 00')" \
  'DFFD role parser'

GATTTOOL_BIN="${test_dir}/fixtures/fake-gatttool-success"
jbl_write_frames '504c011f0000' >/dev/null
aura_write 'aa1304003c0101' 'mock-on' >/dev/null
printf 'PASS strict transport success\n'

GATTTOOL_BIN="${test_dir}/fixtures/fake-gatttool-false-success"
assert_fails 'JBL rc=0 without success text is rejected' jbl_write_frames '504c011f0000'
assert_fails 'Aura rc=0 without success text is rejected' aura_write 'aa1304003c0101' 'mock-on'

set +e
redacted_failure="$(jbl_write_frames '504c011f0000' 2>&1)"
redacted_rc=$?
set -e
((redacted_rc != 0)) || {
  printf 'FAIL failed transport unexpectedly succeeded during redaction test\n' >&2
  exit 1
}
grep -Fq '<bluetooth-address>' <<<"${redacted_failure}" || {
  printf 'FAIL failed transport did not retain a redacted address marker\n' >&2
  exit 1
}
if grep -Fq "${JBL_BT_MAC}" <<<"${redacted_failure}"; then
  printf 'FAIL failed transport leaked the configured address\n' >&2
  exit 1
fi
printf 'PASS transport failure address redaction\n'

saved_jbl_mac="${JBL_BT_MAC}"
JBL_BT_MAC=''
assert_fails 'missing config fails cleanly' validate_config
JBL_BT_MAC="${saved_jbl_mac}"

printf 'All offline tests passed.\n'
