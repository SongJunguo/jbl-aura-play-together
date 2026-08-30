#!/usr/bin/env bash
set -euo pipefail

test_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "${test_dir}/.." && pwd)"
test_tmp="$(mktemp -d)"
trap 'rm -rf -- "${test_tmp}"' EXIT

python_bin="${PYTHON_BIN:-python3}"
manager="${repo_dir}/lib/jbl_aura_session.py"
socket_path="${test_tmp}/runtime/control.sock"
state_path="${test_tmp}/state/session.json"
lock_path="${test_tmp}/runtime/session.lock"
log_path="${test_tmp}/session.log"

export BT_ADAPTER=hci0
export JBL_BT_MAC=02:00:00:00:00:01
export AURA_BT_MAC=02:00:00:00:00:02
export AURA_DEVICE_NAME='Aura Studio 5'
export AURA_TRANSPORT=bredr
export AURA_V4_PID=212d
export JBL_GATT_HANDLE=0x002a
export JBL_GATT_MTU=500
export AURA_GATT_HANDLE=0x03ea
export AURA_GATT_CCCD_HANDLE=0x03ed
export AURA_GATT_PSM=31
export AURA_GATT_MTU=500
export AURA_JOIN_DELAY=0
export SESSION_CONNECT_TIMEOUT=2
export SESSION_AURA_CONNECT_WINDOW=2
export SESSION_AURA_LE_SCAN_WINDOW=1
export SESSION_AURA_LE_RETRIES=0
export SESSION_AURA_LE_RETRY_DELAY=0
export SESSION_WRITE_TIMEOUT=2
export SESSION_AURA_ACK_TIMEOUT=2
export GATTTOOL_BIN="${test_dir}/fixtures/fake-gatttool-session"
export JBL_ENTER_FRAME=504c011f0000
export JBL_START_FRAME=504c151f0c007b22616374696f6e223a317d
export JBL_STOP_FRAME=504c151f0c007b22616374696f6e223a327d
export JBL_EXIT_FRAME=504c021f0000
export AURA_ON_FRAME=aa1304003c0101
export AURA_OFF_FRAME=aa1304003c0100

mkdir -p "$(dirname -- "${log_path}")"
"${python_bin}" "${manager}" daemon \
  --socket "${socket_path}" --state "${state_path}" --lock "${lock_path}" \
  >"${log_path}" 2>&1 &
daemon_pid=$!

for _ in {1..40}; do
  [[ -S "${socket_path}" ]] && break
  kill -0 "${daemon_pid}" 2>/dev/null || {
    printf 'FAIL session daemon exited during startup\n' >&2
    cat "${log_path}" >&2
    exit 1
  }
  sleep 0.05
done
[[ -S "${socket_path}" ]] || {
  printf 'FAIL session socket was not created\n' >&2
  exit 1
}

request() {
  "${python_bin}" "${manager}" client --socket "${socket_path}" --timeout 4 "$1"
}

assert_state() {
  local expected="$1" response="$2" actual
  actual="$(jq -r '.state' <<<"${response}")"
  [[ "${actual}" == "${expected}" ]] || {
    printf 'FAIL expected state %s, got %s\n' "${expected}" "${actual}" >&2
    exit 1
  }
  printf 'PASS managed state %s\n' "${expected}"
}

ready_response="$(request status)"
assert_state ready "${ready_response}"
[[ "$(jq -r '.aura_transport' <<<"${ready_response}")" == 'bredr' ]] || {
  printf 'FAIL manager did not report the mocked BR/EDR transport\n' >&2
  exit 1
}
printf 'PASS mocked BR/EDR transport selection\n'
assert_state linked "$(request start)"
assert_state ready "$(request stop)"
assert_state linked "$(request start)"
assert_state ready "$(request stop)"
assert_state shutting-down "$(request shutdown)"
wait "${daemon_pid}"
[[ ! -S "${socket_path}" ]] || {
  printf 'FAIL session socket survived shutdown\n' >&2
  exit 1
}
assert_state offline "$(cat "${state_path}")"

if rg -n '02:00:00:00:00:0[12]|30323a30303a30303a30303a30303a30' \
  "${log_path}" "${state_path}" >/dev/null; then
  printf 'FAIL session artifacts leaked a Bluetooth address\n' >&2
  exit 1
fi
printf 'PASS session artifacts redact device addresses\n'

set +e
"${python_bin}" "${manager}" client --socket "${socket_path}" --timeout 0.1 status \
  >/dev/null 2>&1
offline_rc=$?
set -e
[[ "${offline_rc}" == 2 ]] || {
  printf 'FAIL offline client returned %s instead of 2\n' "${offline_rc}" >&2
  exit 1
}
printf 'PASS offline session client failure\n'

export FAKE_SESSION_FAIL=aura-off
"${python_bin}" "${manager}" daemon \
  --socket "${socket_path}" --state "${state_path}" --lock "${lock_path}" \
  >>"${log_path}" 2>&1 &
daemon_pid=$!
for _ in {1..40}; do
  [[ -S "${socket_path}" ]] && break
  kill -0 "${daemon_pid}" 2>/dev/null || {
    printf 'FAIL failure-path daemon exited during startup\n' >&2
    exit 1
  }
  sleep 0.05
done
assert_state linked "$(request start)"
set +e
failed_stop_response="$(request stop)"
failed_stop_rc=$?
set -e
[[ "${failed_stop_rc}" == 1 ]] || {
  printf 'FAIL injected stop failure returned %s instead of 1\n' \
    "${failed_stop_rc}" >&2
  exit 1
}
assert_state degraded "${failed_stop_response}"
for _ in {1..60}; do
  ! kill -0 "${daemon_pid}" 2>/dev/null && break
  sleep 0.05
done
set +e
wait "${daemon_pid}"
degraded_daemon_rc=$?
set -e
[[ "${degraded_daemon_rc}" == 1 ]] || {
  printf 'FAIL degraded daemon returned %s instead of 1\n' \
    "${degraded_daemon_rc}" >&2
  exit 1
}
unset FAKE_SESSION_FAIL
[[ ! -S "${socket_path}" ]] || {
  printf 'FAIL degraded session socket survived automatic exit\n' >&2
  exit 1
}
assert_state failed "$(cat "${state_path}")"
printf 'PASS degraded session exits for supervisor restart\n'

"${python_bin}" "${manager}" daemon \
  --socket "${socket_path}" --state "${state_path}" --lock "${lock_path}" \
  >>"${log_path}" 2>&1 &
daemon_pid=$!
for _ in {1..40}; do
  [[ -S "${socket_path}" ]] && break
  kill -0 "${daemon_pid}" 2>/dev/null || {
    printf 'FAIL normalization daemon exited during startup\n' >&2
    exit 1
  }
  sleep 0.05
done
assert_state ready "$(request status)"
assert_state shutting-down "$(request shutdown)"
wait "${daemon_pid}"
grep -Fq 'normalizing roles after an unclean prior session' "${log_path}" || {
  printf 'FAIL restarted daemon did not normalize prior uncertain roles\n' >&2
  exit 1
}
printf 'PASS restart normalizes roles after degraded state\n'

printf '{"state":"offline"}\n' >"${state_path}"
export FAKE_SESSION_DISCONNECT_AFTER_CONNECT=aura
export FAKE_SESSION_DISCONNECT_DELAY=0.8
"${python_bin}" "${manager}" daemon \
  --socket "${socket_path}" --state "${state_path}" --lock "${lock_path}" \
  >>"${log_path}" 2>&1 &
daemon_pid=$!
for _ in {1..40}; do
  [[ -S "${socket_path}" ]] && break
  kill -0 "${daemon_pid}" 2>/dev/null || {
    printf 'FAIL idle-disconnect daemon exited before publishing ready\n' >&2
    exit 1
  }
  sleep 0.05
done
assert_state ready "$(request status)"
for _ in {1..80}; do
  ! kill -0 "${daemon_pid}" 2>/dev/null && break
  sleep 0.05
done
set +e
wait "${daemon_pid}"
disconnect_daemon_rc=$?
set -e
unset FAKE_SESSION_DISCONNECT_AFTER_CONNECT FAKE_SESSION_DISCONNECT_DELAY
[[ "${disconnect_daemon_rc}" == 1 ]] || {
  printf 'FAIL disconnected daemon returned %s instead of 1\n' \
    "${disconnect_daemon_rc}" >&2
  exit 1
}
[[ ! -S "${socket_path}" ]] || {
  printf 'FAIL disconnected ready session kept its socket\n' >&2
  exit 1
}
assert_state failed "$(cat "${state_path}")"
grep -Fq 'control bearer disconnected while state=ready' "${log_path}" || {
  printf 'FAIL idle disconnect was not diagnosed\n' >&2
  exit 1
}
printf 'PASS idle bearer disconnect exits for systemd restart\n'
