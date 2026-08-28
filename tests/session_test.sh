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
export JBL_GATT_HANDLE=0x002a
export JBL_GATT_MTU=500
export AURA_GATT_HANDLE=0x03ea
export AURA_GATT_PSM=31
export AURA_JOIN_DELAY=0
export SESSION_CONNECT_TIMEOUT=2
export SESSION_AURA_CONNECT_WINDOW=2
export SESSION_WRITE_TIMEOUT=2
export SESSION_AURA_ACK_TIMEOUT=2
export GATTTOOL_BIN="${test_dir}/fixtures/fake-gatttool-session"
export JBL_ENTER_FRAME=504c011f0000
export JBL_START_FRAME=504c151f0c007b22616374696f6e223a317d
export JBL_STOP_FRAME=504c151f0c007b22616374696f6e223a327d
export JBL_EXIT_FRAME=504c021f0000
export AURA_ON_FRAME=aa1304003c0101
export AURA_OFF_FRAME=aa1304003c0100
export FAKE_SESSION_CONNECT_FAILS=2

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

assert_state ready "$(request status)"
unset FAKE_SESSION_CONNECT_FAILS
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
assert_state degraded "$(request status)"
kill -TERM "${daemon_pid}"
wait "${daemon_pid}"
unset FAKE_SESSION_FAIL
[[ ! -S "${socket_path}" ]] || {
  printf 'FAIL degraded session socket survived SIGTERM\n' >&2
  exit 1
}
printf 'PASS degraded session shutdown cleanup\n'
