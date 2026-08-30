#!/usr/bin/env bash
# The pactl test double is invoked indirectly through PACTL_BIN.
# shellcheck disable=SC2317,SC2329
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

transport_override="$(
  AURA_TRANSPORT_OVERRIDE=le \
    JBL_AURA_CONFIG="${repo_dir}/config/devices.env.example" \
    bash -c 'source "$1"; printf "%s" "${AURA_TRANSPORT}"' \
      _ "${repo_dir}/bin/jbl-aura-link"
)"
assert_equal 'le' "${transport_override}" \
  'one-shot transport override wins over file configuration'

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
chmod 700 "${lock_runtime}/jbl-aura-link"
printf 'sentinel\n' >"${lock_runtime}/jbl-aura-link/operation.lock"
(
  XDG_RUNTIME_DIR="${lock_runtime}"
  acquire_lock
)
assert_equal 'sentinel' "$(tr -d '\n' <"${lock_runtime}/jbl-aura-link/operation.lock")" \
  'lock open does not truncate an existing file'
assert_equal '700' "$(stat -c '%a' "${lock_runtime}/jbl-aura-link")" \
  'lock directory is private'
acquire_lock
release_operation_lock
(acquire_lock)
printf 'PASS operation lock can be reacquired after explicit release\n'

# Rust holds this same gate for its full service lifetime. Model that owner
# with a separate descriptor and prove every v0.4 public entry fails before it
# can enter its device/session path.
exec 8>>"${SESSION_RUNTIME_DIR}/operation.lock"
flock -n 8
assert_fails 'a Rust owner excludes v0.4 before device I/O' \
  acquire_public_operation_lock
flock -u 8
exec 8>&-
printf 'PASS v0.4 honors the cross-version shared operation lock\n'

acquire_lock
create_launch_reservation
assert_fails 'a secure launch reservation blocks public operations' \
  reject_launch_in_progress
clear_launch_reservation
release_operation_lock
printf 'PASS secure launch reservation create and cleanup\n'

mkdir -m 755 -- "${SESSION_LAUNCH_RESERVATION}"
assert_fails 'wrong-mode launch reservation is rejected' validate_launch_reservation
chmod 700 -- "${SESSION_LAUNCH_RESERVATION}"
clear_launch_reservation
ln -s "${test_tmp}/marker-target" "${SESSION_LAUNCH_RESERVATION}"
assert_fails 'symlinked launch reservation is rejected' validate_launch_reservation
rm -- "${SESSION_LAUNCH_RESERVATION}"

installed_start_events="${test_tmp}/installed-start-events"
(
  installed_ready=0
  prepare_session_paths() { :; }
  export_session_environment() { :; }
  session_available() { ((installed_ready == 1)); }
  systemd_user_available() { return 0; }
  persistent_session_unit_available() { return 0; }
  release_operation_lock() { printf 'release\n' >>"${installed_start_events}"; }
  acquire_lock() { printf 'acquire\n' >>"${installed_start_events}"; }
  systemctl() {
    printf 'systemctl %s\n' "$*" >>"${installed_start_events}"
    installed_ready=1
    clear_launch_reservation
  }
  session_client() { printf '{"ok":true,"state":"ready"}\n'; }
  SESSION_LAUNCH_MODE=systemd
  SESSION_START_TIMEOUT=1
  launch_session_daemon >/dev/null
)
assert_equal $'release\nsystemctl --user start jbl-aura-link-session.service\nacquire' \
  "$(cat "${installed_start_events}")" \
  'installed service start releases and reacquires the outer operation lock'

# Exercise the boot/restart path as separate processes: ExecStartPre must leave
# the reservation behind after its operation-lock fd closes, and ExecStartPost
# may remove it only after the managed session is ready.
(
  INVOCATION_ID=11111111111111111111111111111111
  validate_config() { :; }
  require_command() { :; }
  disconnect_aura_a2dp() { :; }
  begin_pulse_bluetooth_guard() { :; }
  BLUEZ_FULL_DISCONNECT_BEFORE_SESSION=false
  service_preflight >/dev/null
)
[[ -d "${SESSION_LAUNCH_RESERVATION}" ]] || {
  printf 'FAIL automatic service preflight did not retain its launch reservation\n' >&2
  exit 1
}
set +e
(acquire_public_operation_lock) >/dev/null 2>&1
preflight_writer_rc=$?
set -e
((preflight_writer_rc != 0)) || {
  printf 'FAIL public writer crossed the post-preflight launch window\n' >&2
  exit 1
}
(
  INVOCATION_ID=11111111111111111111111111111111
  require_session_manager() { :; }
  require_command() { :; }
  wait_for_managed_session() { return 0; }
  restore_pulse_bluetooth_modules() { :; }
  service_post_start
)
[[ ! -e "${SESSION_LAUNCH_RESERVATION}" ]] || {
  printf 'FAIL successful service post-start retained its launch reservation\n' >&2
  exit 1
}
printf 'PASS automatic service launch reserves preflight through ready post-start\n'

set +e
(
  INVOCATION_ID=22222222222222222222222222222222
  validate_config() { :; }
  require_command() { :; }
  disconnect_aura_a2dp() { return 1; }
  restore_pulse_bluetooth_modules() { :; }
  restore_aura_a2dp() { :; }
  service_preflight
) >/dev/null 2>&1
failed_preflight_rc=$?
set -e
((failed_preflight_rc != 0)) || {
  printf 'FAIL mocked service preflight failure unexpectedly succeeded\n' >&2
  exit 1
}
[[ ! -e "${SESSION_LAUNCH_RESERVATION}" ]] || {
  printf 'FAIL failed service preflight retained its launch reservation\n' >&2
  exit 1
}
printf 'PASS failed service preflight clears its launch reservation\n'

# A delayed ExecStopPost from an older systemd runtime cycle must not consume
# the reservation of a newly queued start transaction.
acquire_lock
create_launch_reservation
INVOCATION_ID=33333333333333333333333333333333 \
  claim_launch_reservation_for_service
release_operation_lock
(
  INVOCATION_ID=44444444444444444444444444444444
  restore_pulse_bluetooth_modules() { :; }
  restore_aura_a2dp() { :; }
  SERVICE_RESULT=success
  service_cleanup
)
[[ -d "${SESSION_LAUNCH_RESERVATION}/invocation-33333333333333333333333333333333" ]] || {
  printf 'FAIL old service cleanup consumed the next invocation reservation\n' >&2
  exit 1
}
(
  INVOCATION_ID=33333333333333333333333333333333
  restore_pulse_bluetooth_modules() { :; }
  restore_aura_a2dp() { :; }
  SERVICE_RESULT=success
  service_cleanup
)
[[ ! -e "${SESSION_LAUNCH_RESERVATION}" ]] || {
  printf 'FAIL owning service cleanup retained its launch reservation\n' >&2
  exit 1
}
printf 'PASS service cleanup only releases its own invocation reservation\n'

concurrent_ready="${test_tmp}/launch-reservation-ready"
concurrent_release="${test_tmp}/launch-reservation-release"
concurrent_device_log="${test_tmp}/launch-reservation-device-writes"
(
  acquire_lock
  create_launch_reservation
  release_operation_lock
  : >"${concurrent_ready}"
  while [[ ! -e "${concurrent_release}" ]]; do
    sleep 0.01
  done
  acquire_lock
  clear_launch_reservation
  release_operation_lock
) &
reservation_holder_pid=$!
for _ in {1..200}; do
  [[ -e "${concurrent_ready}" ]] && break
  sleep 0.01
done
[[ -e "${concurrent_ready}" ]] || {
  kill "${reservation_holder_pid}" >/dev/null 2>&1 || true
  wait "${reservation_holder_pid}" >/dev/null 2>&1 || true
  printf 'FAIL concurrent launch reservation holder did not become ready\n' >&2
  exit 1
}
set +e
(
  validate_config() { :; }
  require_session_manager() { :; }
  require_command() { :; }
  session_available() {
    printf 'UNEXPECTED_DEVICE_TRANSACTION\n' >>"${concurrent_device_log}"
    return 0
  }
  session_client() {
    printf 'UNEXPECTED_DEVICE_TRANSACTION\n' >>"${concurrent_device_log}"
    return 1
  }
  stop_link
) >/dev/null 2>&1
concurrent_writer_rc=$?
set -e
((concurrent_writer_rc != 0)) || {
  printf 'FAIL concurrent public writer crossed the launch reservation\n' >&2
  exit 1
}
[[ ! -e "${concurrent_device_log}" ]] || {
  printf 'FAIL concurrent public writer reached a device/session operation\n' >&2
  exit 1
}
: >"${concurrent_release}"
wait "${reservation_holder_pid}"
acquire_public_operation_lock
release_operation_lock
printf 'PASS concurrent public writer is blocked for the full reserved launch window\n'

acquire_lock
create_launch_reservation
release_operation_lock
public_guard_log="${test_tmp}/public-entry-device-operations"
for guarded_entry in start_link stop_link shutdown_session recover_stop_link \
  install_user_service show_status; do
  set +e
  (
    validate_config() { :; }
    require_command() { :; }
    require_gatttool() { :; }
    require_session_manager() { :; }
    require_dbus_fast() { :; }
    systemd_user_available() { return 0; }
    session_available() {
      printf 'UNEXPECTED:%s\n' "${guarded_entry}" >>"${public_guard_log}"
      return 0
    }
    session_client() {
      printf 'UNEXPECTED:%s\n' "${guarded_entry}" >>"${public_guard_log}"
      return 1
    }
    "${guarded_entry}"
  ) >/dev/null 2>&1
  guarded_entry_rc=$?
  set -e
  ((guarded_entry_rc != 0)) || {
    printf 'FAIL %s crossed an active launch reservation\n' "${guarded_entry}" >&2
    exit 1
  }
done
[[ ! -e "${public_guard_log}" ]] || {
  printf 'FAIL a guarded public entry reached a session/device operation\n' >&2
  exit 1
}
acquire_lock
clear_launch_reservation
release_operation_lock
printf 'PASS every public control entry fails closed during reserved launch\n'

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
[[ -f "${PULSE_GUARD_STATE}" ]] || {
  printf 'FAIL PulseAudio guard did not persist crash-recovery state\n' >&2
  exit 1
}
printf 'PASS PulseAudio guard persists crash-recovery state\n'
# Simulate ExecStartPre exiting before ExecStartPost: the next process has only
# the private state file, not the original Bash arrays.
PULSE_BT_GUARD_ACTIVE=0
PULSE_BT_MODULE_IDS=()
PULSE_BT_MODULE_NAMES=()
PULSE_BT_MODULE_ARGS=()
PULSE_BT_MODULE_UNLOADED=()
restore_pulse_bluetooth_modules >/dev/null
assert_equal '2' "${fake_pactl_loads}" 'PulseAudio guard restore count'
assert_equal '1' "${fake_policy_active}" 'PulseAudio policy restored state'
assert_equal '1' "${fake_discover_active}" 'PulseAudio discover restored state'
assert_equal 'auto_switch=2' "${fake_policy_loaded_args}" \
  'PulseAudio policy args restored'
assert_equal 'headset="native hfp"' "${fake_discover_loaded_args}" \
  'PulseAudio discover args restored'
[[ ! -e "${PULSE_GUARD_STATE}" ]] || {
  printf 'FAIL PulseAudio guard state survived successful restore\n' >&2
  exit 1
}
printf 'PASS PulseAudio persisted state clears after restore\n'

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

shutdown_event_log="${test_tmp}/shutdown-events"
set +e
shutdown_pending_output="$(
  (
    acquire_lock() { :; }
    session_available() { return 0; }
    session_client() {
      printf '%s\n' "$1" >>"${shutdown_event_log}"
      case "$1" in
        stop) printf '{"ok":true,"state":"ready"}\n' ;;
        shutdown) printf '{"ok":true,"state":"shutting-down"}\n' ;;
        *) return 1 ;;
      esac
    }
    restore_aura_a2dp() {
      printf 'restore-a2dp\n' >>"${shutdown_event_log}"
      return 1
    }
    shutdown_session
  ) 2>&1
)"
shutdown_pending_rc=$?
set -e
assert_equal '0' "${shutdown_pending_rc}" \
  'shutdown succeeds when only optional A2DP restoration is pending'
assert_equal $'stop\nshutdown\nrestore-a2dp' "$(cat "${shutdown_event_log}")" \
  'shutdown releases control before attempting A2DP restoration'
grep -Fq 'A2DP restoration remains pending' <<<"${shutdown_pending_output}" || {
  printf 'FAIL shutdown did not report pending A2DP restoration\n' >&2
  exit 1
}
printf 'PASS shutdown reports pending optional A2DP restoration\n'

failure_cleanup_output="$(
  (
    restore_pulse_bluetooth_modules() { printf 'RESTORE_PULSE\n'; }
    restore_aura_a2dp() { printf 'RESTORE_A2DP\n'; }
    SERVICE_RESULT=exit-code
    service_cleanup
  )
)"
grep -Fq 'RESTORE_PULSE' <<<"${failure_cleanup_output}" || {
  printf 'FAIL service failure cleanup did not restore PulseAudio\n' >&2
  exit 1
}
if grep -Fq 'RESTORE_A2DP' <<<"${failure_cleanup_output}"; then
  printf 'FAIL automatic failure recovery restored competing A2DP\n' >&2
  exit 1
fi
printf 'PASS automatic failure recovery skips competing A2DP restoration\n'
success_cleanup_output="$(
  (
    restore_pulse_bluetooth_modules() { :; }
    restore_aura_a2dp() { printf 'RESTORE_A2DP\n'; }
    SERVICE_RESULT=success
    service_cleanup
  )
)"
grep -Fq 'RESTORE_A2DP' <<<"${success_cleanup_output}" || {
  printf 'FAIL graceful service cleanup did not restore prior A2DP\n' >&2
  exit 1
}
printf 'PASS graceful service cleanup restores prior A2DP\n'

(
  acquire_lock() { :; }
  disconnect_aura_a2dp() { :; }
  begin_pulse_bluetooth_guard() { :; }
  restore_pulse_bluetooth_modules() { :; }
  release_aura_bluez_session() { :; }
  restore_aura_a2dp() { :; }
  GATTTOOL_BIN="${test_dir}/fixtures/fake-gatttool-session"
  AURA_TRANSPORT=bredr
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

service_root="${test_tmp}/service-install"
service_log="${service_root}/systemctl.log"
mkdir -p "${service_root}"
JBL_AURA_INSTALL_ROOT="${service_root}/lib/jbl-aura-link" \
JBL_AURA_USER_BIN_DIR="${service_root}/bin" \
JBL_AURA_USER_UNIT_DIR="${service_root}/units" \
JBL_AURA_CONFIG="${repo_dir}/config/devices.env.example" \
SERVICE_TEST_LOG="${service_log}" \
bash -c '
  set -euo pipefail
  source "$1"
  systemd_user_available() { return 0; }
  session_available() { return 1; }
  wait_for_managed_session() { return 0; }
  systemctl() {
    printf "%s\n" "$*" >>"${SERVICE_TEST_LOG}"
    [[ "$*" != "--user is-enabled --quiet jbl-aura-link-rust.service" ]]
  }
  loginctl() { printf "yes\n"; }
  install_user_service >/dev/null
' _ "${repo_dir}/bin/jbl-aura-link"
[[ -L "${service_root}/bin/jbl-aura-link" ]] || {
  printf 'FAIL install-service did not create the simple launcher\n' >&2
  exit 1
}
printf 'PASS install-service creates the simple launcher\n'
resolved_manager="$(
  JBL_AURA_CONFIG="${repo_dir}/config/devices.env.example" \
    bash -c 'source "$1"; printf "%s" "${JBL_AURA_SESSION_MANAGER}"' \
      _ "${service_root}/bin/jbl-aura-link"
)"
resolved_manager="$(readlink -m -- "${resolved_manager}")"
assert_equal "${service_root}/lib/jbl-aura-link/lib/jbl_aura_session.py" \
  "${resolved_manager}" 'installed launcher resolves its real manager path'
service_unit="${service_root}/units/jbl-aura-link-session.service"
grep -Fq 'Restart=on-failure' "${service_unit}" || {
  printf 'FAIL installed unit does not retry failed cold startup\n' >&2
  exit 1
}
if ! grep -Fq 'ExecStartPre=' "${service_unit}" ||
  ! grep -Fq 'ExecStartPost=' "${service_unit}" ||
  ! grep -Fq 'ExecStopPost=' "${service_unit}"; then
    printf 'FAIL installed unit lacks guarded lifecycle hooks\n' >&2
    exit 1
fi
if rg -n '@[A-Z_]+@' "${service_unit}" >/dev/null; then
  printf 'FAIL installed unit retained a template token\n' >&2
  exit 1
fi
printf 'PASS install-service renders guarded boot unit\n'
if ! grep -Fq -- '--user enable jbl-aura-link-session.service' "${service_log}" ||
  ! grep -Fq -- '--user restart jbl-aura-link-session.service' "${service_log}"; then
    printf 'FAIL install-service did not enable and restart the unit\n' >&2
    exit 1
fi
printf 'PASS install-service enables and starts the boot unit\n'

v04_conflict_log="${service_root}/v04-conflict.log"
set +e
JBL_AURA_INSTALL_ROOT="${service_root}/conflict/lib/jbl-aura-link" \
JBL_AURA_USER_BIN_DIR="${service_root}/conflict/bin" \
JBL_AURA_USER_UNIT_DIR="${service_root}/conflict/units" \
JBL_AURA_CONFIG="${repo_dir}/config/devices.env.example" \
SERVICE_TEST_LOG="${v04_conflict_log}" \
bash -c '
  set -euo pipefail
  source "$1"
  systemd_user_available() { return 0; }
  systemctl() {
    printf "%s\n" "$*" >>"${SERVICE_TEST_LOG}"
    [[ "$*" == "--user is-enabled --quiet jbl-aura-link-rust.service" ]]
  }
  install_user_service
' _ "${repo_dir}/bin/jbl-aura-link" >/dev/null 2>&1
v04_conflict_rc=$?
set -e
if ((v04_conflict_rc == 0)); then
  printf 'FAIL v0.4 installer accepted an enabled Rust unit\n' >&2
  exit 1
fi
[[ ! -e "${service_root}/conflict/bin/jbl-aura-link" ]] || {
  printf 'FAIL v0.4 installer changed files before rejecting Rust ownership\n' >&2
  exit 1
}
printf 'PASS v0.4 installer rejects an enabled Rust unit before mutation\n'

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
