#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
readonly action_timeout_seconds=600
test_root=$(mktemp -d)
cleanup() {
    rm -rf -- "${test_root}"
}
trap cleanup EXIT

test_home="${test_root}/home"
fake_bin="${test_root}/fake-bin"
fixture_binary="${test_root}/jbl-aura-link"
systemctl_log="${test_root}/systemctl.log"
mkdir -m 0700 -- "${test_home}"
mkdir -m 0755 -- "${test_home}/.local" "${fake_bin}"
mkdir -m 0755 -- "${test_home}/.local/bin"
printf '%s\n' 'legacy-v0.4-sentinel' >"${test_home}/.local/bin/jbl-aura-link"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' >"${fixture_binary}"
chmod 0755 "${fixture_binary}"

# The generated fixture must expand these variables when it runs, not now.
# shellcheck disable=SC2016
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'printf '\''%s\n'\'' "$*" >>"${SYSTEMCTL_TEST_LOG}"' \
    'if [[ "$*" == "--user is-enabled --quiet jbl-aura-link-session.service" ]]; then' \
    '  [[ "${LEGACY_UNIT_ENABLED:-0}" == 1 ]]' \
    '  exit' \
    'fi' \
    >"${fake_bin}/systemctl"
chmod 0755 "${fake_bin}/systemctl"

HOME="${test_home}" \
XDG_CONFIG_HOME="${test_home}/.config" \
PATH="${fake_bin}:/usr/bin:/bin" \
SYSTEMCTL_TEST_LOG="${systemctl_log}" \
    "${repo_root}/scripts/install-rust-user-service.sh" --binary "${fixture_binary}" >/dev/null

grep -Fxq 'legacy-v0.4-sentinel' "${test_home}/.local/bin/jbl-aura-link"
test -x "${test_home}/.local/bin/jbl-aura-link-rust"
unit="${test_home}/.config/systemd/user/jbl-aura-link-rust.service"
test -f "${unit}"
grep -Fxq 'ExecStart=%h/.local/bin/jbl-aura-link-rust --config %h/.config/jbl-aura-link-rust/devices.env serve' "${unit}"
grep -Fxq 'Restart=on-failure' "${unit}"
grep -Fxq "TimeoutStopSec=${action_timeout_seconds}s" "${unit}"
grep -Fxq "const ACTION_TIMEOUT_SECONDS: u64 = ${action_timeout_seconds};" \
    "${repo_root}/rust/src/local_client.rs"
grep -Fxq 'ProtectSystem=strict' "${unit}"
grep -Fxq 'StateDirectory=jbl-aura-link-rust' "${unit}"
grep -Fxq 'StateDirectoryMode=0700' "${unit}"
grep -Fxq 'RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_BLUETOOTH' "${unit}"
grep -Fxq -- '--user daemon-reload' "${systemctl_log}"
if grep -Fq -- '--user enable' "${systemctl_log}"; then
    echo 'installer unexpectedly enabled before explicit authorization' >&2
    exit 1
fi
rust_config="${test_home}/.config/jbl-aura-link-rust/devices.env"
test -f "${rust_config}"
test "$(stat -c '%a' "${rust_config}")" = 600
grep -Fxq 'JBL_EXPECTED_MODEL=JBL Authentics 300' "${rust_config}"
test "$(stat -c '%a' "${test_home}/.local/state/jbl-aura-link-rust")" = 700
if grep -Eq -- '--now| start ' "${systemctl_log}"; then
    echo 'installer unexpectedly started a service' >&2
    exit 1
fi

printf '%s\n' '# operator-value-preserved' >>"${rust_config}"
HOME="${test_home}" \
XDG_CONFIG_HOME="${test_home}/.config" \
PATH="${fake_bin}:/usr/bin:/bin" \
SYSTEMCTL_TEST_LOG="${systemctl_log}" \
    "${repo_root}/scripts/install-rust-user-service.sh" \
        --binary "${fixture_binary}" --enable >/dev/null
grep -Fxq -- '--user enable jbl-aura-link-rust.service' "${systemctl_log}"
grep -Fxq 'legacy-v0.4-sentinel' "${test_home}/.local/bin/jbl-aura-link"
grep -Fxq '# operator-value-preserved' "${rust_config}"

conflict_home="${test_root}/conflict-home"
mkdir -m 0700 -- "${conflict_home}"
set +e
HOME="${conflict_home}" \
XDG_CONFIG_HOME="${conflict_home}/.config" \
PATH="${fake_bin}:/usr/bin:/bin" \
SYSTEMCTL_TEST_LOG="${systemctl_log}" \
LEGACY_UNIT_ENABLED=1 \
    "${repo_root}/scripts/install-rust-user-service.sh" \
        --binary "${fixture_binary}" --enable >/dev/null 2>&1
conflict_rc=$?
set -e
if ((conflict_rc == 0)); then
    echo 'installer enabled Rust while the v0.4 unit was enabled' >&2
    exit 1
fi
test ! -e "${conflict_home}/.local/bin/jbl-aura-link-rust"

echo 'rust service installer tests passed'
