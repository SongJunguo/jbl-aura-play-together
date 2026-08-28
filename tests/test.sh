#!/usr/bin/env bash
set -euo pipefail

test_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "${test_dir}/.." && pwd)"
test_tmp="$(mktemp -d)"
trap 'rm -rf -- "${test_tmp}"' EXIT
export XDG_RUNTIME_DIR="${test_tmp}/xdg-runtime"
export XDG_STATE_HOME="${test_tmp}/xdg-state"
mkdir -p "${XDG_RUNTIME_DIR}" "${XDG_STATE_HOME}"

export JBL_AURA_CONFIG="${repo_dir}/config/devices.env.example"
export JBL_CONNECT_DELAY=0
export JBL_STEP_DELAY=0
export JBL_GATT_TIMEOUT=3
export AURA_GATT_TIMEOUT=3
export AURA_GATT_RETRIES=1

# shellcheck source-path=SCRIPTDIR
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

lock_runtime="${test_tmp}/runtime"
mkdir -p "${lock_runtime}/jbl-aura-link"
printf 'sentinel\n' >"${lock_runtime}/jbl-aura-link/operation.lock"
(
  XDG_RUNTIME_DIR="${lock_runtime}"
  acquire_lock
)
assert_equal 'sentinel' "$(tr -d '\n' <"${lock_runtime}/jbl-aura-link/operation.lock")" \
  'lock open does not truncate an existing file'
assert_equal '700' "$(stat -c '%a' "${lock_runtime}/jbl-aura-link")" \
  'lock directory is private'

symlink_runtime="${test_tmp}/symlink-runtime"
mkdir -p "${symlink_runtime}/target"
ln -s "${symlink_runtime}/target" "${symlink_runtime}/jbl-aura-link"
set +e
(
  XDG_RUNTIME_DIR="${symlink_runtime}"
  acquire_lock
) >/dev/null 2>&1
symlink_lock_rc=$?
set -e
assert_equal '1' "${symlink_lock_rc}" 'symlinked lock directory is rejected'

fake_bluetooth_path="${test_tmp}/fake-bluetooth-path"
mkdir -p "${fake_bluetooth_path}"
ln -s "${test_dir}/fixtures/fake-bluetoothctl-hang" \
  "${fake_bluetooth_path}/bluetoothctl"
saved_path="${PATH}"
saved_bluez_timeout="${BLUEZ_CONTROL_TIMEOUT}"
saved_aura_settle="${AURA_CONTROL_SETTLE}"
PATH="${fake_bluetooth_path}:${PATH}"
BLUEZ_CONTROL_TIMEOUT=0.1
AURA_CONTROL_SETTLE=0
SECONDS=0
release_aura_bluez_session
if ((SECONDS >= 2)); then
  printf 'FAIL BlueZ release was not bounded by its timeout\n' >&2
  exit 1
fi
printf 'PASS BlueZ release timeout\n'
PATH="${saved_path}"
BLUEZ_CONTROL_TIMEOUT="${saved_bluez_timeout}"
AURA_CONTROL_SETTLE="${saved_aura_settle}"

reset_fake_pactl() {
  fake_policy_active=1
  fake_discover_active=1
  fake_policy_args='auto_switch=2'
  fake_discover_args='headset="native hfp"'
  fake_pactl_unloads=0
  fake_pactl_loads=0
  fake_policy_loads=0
  fake_discover_loads=0
  fake_policy_loaded_args=''
  fake_discover_loaded_args=''
  fake_fail_unload_id=''
  fake_fail_load_name=''
  fake_fail_load_remaining=0
  fake_fail_list=0
  fake_emit_operations=0
}

# Invoked indirectly through PACTL_BIN by the sourced production script.
# shellcheck disable=SC2329
pactl() {
  case "${1:-}" in
    list)
      [[ "${2:-} ${3:-}" == 'modules short' ]] || return 1
      ((fake_fail_list == 0)) || return 1
      ((fake_policy_active == 1)) &&
        printf '11\tmodule-bluetooth-policy\t%s\t\n' "${fake_policy_args}"
      ((fake_discover_active == 1)) &&
        printf '12\tmodule-bluetooth-discover\t%s\t\n' "${fake_discover_args}"
      return 0
      ;;
    unload-module)
      fake_pactl_unloads=$((fake_pactl_unloads + 1))
      if [[ "${2:-}" == "${fake_fail_unload_id}" ]]; then
        return 1
      fi
      case "${2:-}" in
        11) fake_policy_active=0 ;;
        12) fake_discover_active=0 ;;
        *) return 1 ;;
      esac
      ;;
    load-module)
      fake_pactl_loads=$((fake_pactl_loads + 1))
      ((fake_emit_operations == 0)) ||
        printf 'FAKE_LOAD:%s:<%s>\n' "${2:-}" "${3:-}" >&2
      case "${2:-}" in
        module-bluetooth-policy)
          fake_policy_loads=$((fake_policy_loads + 1))
          fake_policy_loaded_args="${3:-}"
          ;;
        module-bluetooth-discover)
          fake_discover_loads=$((fake_discover_loads + 1))
          fake_discover_loaded_args="${3:-}"
          ;;
        *) return 1 ;;
      esac
      if [[ "${2:-}" == "${fake_fail_load_name}" ]] &&
        ((fake_fail_load_remaining > 0)); then
        fake_fail_load_remaining=$((fake_fail_load_remaining - 1))
        return 1
      fi
      case "${2:-}" in
        module-bluetooth-policy) fake_policy_active=1 ;;
        module-bluetooth-discover) fake_discover_active=1 ;;
      esac
      printf '99\n'
      ;;
    *)
      printf 'unexpected fake pactl call: %s\n' "$*" >&2
      return 1
      ;;
  esac
}

saved_pactl_bin="${PACTL_BIN}"
saved_pulse_guard="${PULSEAUDIO_BLUETOOTH_GUARD}"
PACTL_BIN='pactl'
PULSEAUDIO_BLUETOOTH_GUARD='auto'
reset_fake_pactl
begin_pulse_bluetooth_guard >/dev/null
assert_equal '2' "${fake_pactl_unloads}" 'PulseAudio guard unload count'
assert_equal '0' "${fake_policy_active}" 'PulseAudio policy suspended state'
assert_equal '0' "${fake_discover_active}" 'PulseAudio discover suspended state'
assert_equal 'auto_switch=2' "${PULSE_BT_MODULE_ARGS[0]}" \
  'PulseAudio policy args captured'
assert_equal 'headset="native hfp"' "${PULSE_BT_MODULE_ARGS[1]}" \
  'PulseAudio discover args captured'
restore_pulse_bluetooth_modules >/dev/null
assert_equal '2' "${fake_pactl_loads}" 'PulseAudio guard restore count'
assert_equal '1' "${fake_policy_active}" 'PulseAudio policy restored state'
assert_equal '1' "${fake_discover_active}" 'PulseAudio discover restored state'
assert_equal 'auto_switch=2' "${fake_policy_loaded_args}" \
  'PulseAudio policy args restored'
assert_equal 'headset="native hfp"' "${fake_discover_loaded_args}" \
  'PulseAudio discover args restored'

reset_fake_pactl
fake_fail_unload_id=12
set +e
suspend_pulse_bluetooth_modules >/dev/null 2>&1
partial_unload_rc=$?
set -e
assert_equal '1' "${partial_unload_rc}" 'PulseAudio partial unload fails'
assert_equal '1' "${fake_policy_active}" 'PulseAudio partial unload restores policy'
assert_equal '1' "${fake_discover_active}" 'PulseAudio failed unload leaves discover active'
assert_equal '0' "${PULSE_BT_GUARD_ACTIVE}" 'PulseAudio partial unload closes guard'
assert_equal 'auto_switch=2' "${fake_policy_loaded_args}" \
  'PulseAudio partial unload restores original args'

reset_fake_pactl
begin_pulse_bluetooth_guard >/dev/null
fake_fail_load_name='module-bluetooth-discover'
fake_fail_load_remaining=1
set +e
restore_pulse_bluetooth_modules >/dev/null 2>&1
first_restore_rc=$?
set -e
assert_equal '1' "${first_restore_rc}" 'PulseAudio first restore failure is reported'
assert_equal '1' "${PULSE_BT_GUARD_ACTIVE}" 'PulseAudio failed restore keeps guard active'
assert_equal '0' "${PULSE_BT_MODULE_UNLOADED[0]}" 'PulseAudio restored module is cleared'
assert_equal '1' "${PULSE_BT_MODULE_UNLOADED[1]}" 'PulseAudio missing module remains pending'
restore_pulse_bluetooth_modules >/dev/null
assert_equal '0' "${PULSE_BT_GUARD_ACTIVE}" 'PulseAudio retry closes guard'
assert_equal '1' "${fake_policy_loads}" 'PulseAudio retry does not duplicate restored policy'
assert_equal '2' "${fake_discover_loads}" 'PulseAudio retry reloads only missing discover'
assert_equal '1' "${fake_discover_active}" 'PulseAudio retry restores discover'

reset_fake_pactl
fake_fail_list=1
set +e
begin_pulse_bluetooth_guard >/dev/null 2>&1
pulse_probe_rc=$?
set -e
assert_equal '1' "${pulse_probe_rc}" 'PulseAudio auto guard reports pactl query failure'

set +e
stop_term_output="$(
  (
    set -e
    reset_fake_pactl
    fake_emit_operations=1
    acquire_lock() { :; }
    disconnect_aura_a2dp() { printf 'FAKE_A2DP_SNAPSHOT\n'; }
    restore_aura_a2dp() { printf 'FAKE_A2DP_RESTORE\n'; }
    release_aura_bluez_session() {
      kill -TERM "${BASHPID}"
      printf 'UNREACHABLE_AFTER_TERM\n'
    }
    recover_stop_link
  ) 2>&1
)"
stop_term_rc=$?
set -e
assert_equal '143' "${stop_term_rc}" 'recovery stop TERM returns 143'
grep -Fq 'FAKE_A2DP_SNAPSHOT' <<<"${stop_term_output}" || {
  printf 'FAIL stop did not snapshot A2DP before release\n' >&2
  exit 1
}
printf 'PASS recovery stop snapshots A2DP before release\n'
grep -Fq 'FAKE_A2DP_RESTORE' <<<"${stop_term_output}" || {
  printf 'FAIL stop TERM did not restore A2DP state\n' >&2
  exit 1
}
printf 'PASS recovery stop TERM restores A2DP state\n'
if grep -Fq 'UNREACHABLE_AFTER_TERM' <<<"${stop_term_output}"; then
  printf 'FAIL stop continued after TERM\n' >&2
  exit 1
fi
printf 'PASS recovery stop TERM does not continue transaction\n'
assert_equal '2' "$(grep -c '^FAKE_LOAD:' <<<"${stop_term_output}" || true)" \
  'recovery stop TERM restores both PulseAudio modules'

set +e
a2dp_disabled_output="$(
  (
    DISCONNECT_AURA_A2DP=false
    busctl() { printf 'UNEXPECTED_BUSCTL_CALL\n'; }
    aura_a2dp_active() { return 0; }
    disconnect_aura_a2dp
  ) 2>&1
)"
a2dp_disabled_rc=$?
set -e
assert_equal '1' "${a2dp_disabled_rc}" 'active A2DP fails when disconnect is disabled'
if grep -Fq 'UNEXPECTED_BUSCTL_CALL' <<<"${a2dp_disabled_output}"; then
  printf 'FAIL disabled A2DP path still changed BlueZ state\n' >&2
  exit 1
fi

unset -f pactl
PACTL_BIN="${saved_pactl_bin}"
PULSEAUDIO_BLUETOOTH_GUARD="${saved_pulse_guard}"

(
  acquire_lock() { :; }
  disconnect_aura_a2dp() { :; }
  begin_pulse_bluetooth_guard() { :; }
  restore_pulse_bluetooth_modules() { :; }
  release_aura_bluez_session() { :; }
  restore_aura_a2dp() { :; }
  GATTTOOL_BIN="${test_dir}/fixtures/fake-gatttool-session"
  AURA_JOIN_DELAY=0
  SESSION_START_TIMEOUT=5
  SESSION_CONNECT_TIMEOUT=2
  SESSION_WRITE_TIMEOUT=2
  SESSION_AURA_ACK_TIMEOUT=2
  SESSION_LAUNCH_MODE=direct
  start_link >/dev/null
  assert_equal 'linked' \
    "$(session_client status 2 | jq -r '.state')" \
    'Bash wrapper starts managed session'
  stop_link >/dev/null
  assert_equal 'ready' \
    "$(session_client status 2 | jq -r '.state')" \
    'Bash wrapper stops through held session'
  start_link >/dev/null
  shutdown_session >/dev/null
  [[ ! -S "${SESSION_SOCKET}" ]] || {
    printf 'FAIL Bash wrapper shutdown kept session socket\n' >&2
    exit 1
  }
  printf 'PASS Bash wrapper persistent cycle and shutdown\n'
)

set +e
managed_stop_output="$(stop_link 2>&1)"
managed_stop_rc=$?
set -e
assert_equal '1' "${managed_stop_rc}" 'managed stop refuses without a held session'
grep -Fq 'no managed control session' <<<"${managed_stop_output}" || {
  printf 'FAIL managed stop did not explain the missing session\n' >&2
  exit 1
}
printf 'PASS managed stop explains missing held session\n'

saved_jbl_mac="${JBL_BT_MAC}"
JBL_BT_MAC=''
assert_fails 'missing config fails cleanly' validate_config
JBL_BT_MAC="${saved_jbl_mac}"

saved_pulse_guard="${PULSEAUDIO_BLUETOOTH_GUARD}"
PULSEAUDIO_BLUETOOTH_GUARD='invalid'
assert_fails 'invalid PulseAudio guard mode is rejected' validate_config
PULSEAUDIO_BLUETOOTH_GUARD="${saved_pulse_guard}"

saved_bluez_timeout="${BLUEZ_CONTROL_TIMEOUT}"
BLUEZ_CONTROL_TIMEOUT=0
assert_fails 'zero BlueZ control timeout is rejected' validate_config
BLUEZ_CONTROL_TIMEOUT="${saved_bluez_timeout}"

printf 'All offline tests passed.\n'
