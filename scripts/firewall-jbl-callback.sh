#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

readonly RULE_COMMENT='jbl-aura-7951'
readonly DEFAULT_CALLBACK_PORT='8098'

fail() {
  printf 'firewall-jbl-callback: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: sudo scripts/firewall-jbl-callback.sh <install|remove|status> [--config ABSOLUTE_PATH]

Installs only this rule:
  configured JBL IPv4 -> fixed local TCP callback port

It never enables/disables UFW and never opens the port to a subnet or the
public Internet. `remove` is the exact rollback operation.
EOF
}

default_config() {
  local owner_name owner_home
  owner_name="${SUDO_USER:-}"
  [[ -n "${owner_name}" && "${owner_name}" != root ]] ||
    fail 'pass --config when not running through sudo from the target user'
  owner_home="$(getent passwd "${owner_name}" | cut -d: -f6)"
  [[ -n "${owner_home}" && "${owner_home}" == /* ]] ||
    fail 'could not resolve the target user home directory'
  printf '%s/.config/jbl-aura-link-rust/devices.env' "${owner_home}"
}

valid_ipv4() {
  local value="$1" octet
  local -a octets
  [[ "${value}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 1
  IFS=. read -r -a octets <<<"${value}"
  ((${#octets[@]} == 4)) || return 1
  for octet in "${octets[@]}"; do
    [[ "${octet}" =~ ^(0|[1-9][0-9]{0,2})$ ]] || return 1
    ((10#${octet} <= 255)) || return 1
  done
  [[ "${value}" != '0.0.0.0' && "${value}" != '255.255.255.255' ]]
}

load_config() {
  local path="$1" key value mode
  [[ "${path}" == /* && -f "${path}" && ! -L "${path}" ]] ||
    fail 'config must be an existing absolute regular file, not a symlink'
  mode="$(stat -c '%a' -- "${path}")"
  [[ "${mode}" == 600 || "${mode}" == 400 ]] ||
    fail 'config must be owner-only (mode 0600 or 0400)'

  JBL_IP=''
  JBL_GENA_CALLBACK_PORT="${DEFAULT_CALLBACK_PORT}"
  while IFS='=' read -r key value || [[ -n "${key:-}" ]]; do
    [[ -n "${key}" && "${key}" != \#* ]] || continue
    value="${value%$'\r'}"
    case "${key}" in
      JBL_IP) JBL_IP="${value}" ;;
      JBL_GENA_CALLBACK_PORT) JBL_GENA_CALLBACK_PORT="${value}" ;;
    esac
  done <"${path}"

  valid_ipv4 "${JBL_IP}" || fail 'configured JBL_IP is not a valid unicast IPv4 address'
  [[ "${JBL_GENA_CALLBACK_PORT}" =~ ^[0-9]+$ ]] ||
    fail 'callback port must be an integer'
  ((JBL_GENA_CALLBACK_PORT >= 1024 && JBL_GENA_CALLBACK_PORT <= 65535)) ||
    fail 'callback port must be in 1024..65535'
}

rule_present() {
  ufw status 2>/dev/null | awk \
    -v source="${JBL_IP}" -v port="${JBL_GENA_CALLBACK_PORT}/tcp" '
      index($0, source) && index($0, port) && index($0, "ALLOW") { found = 1 }
      END { exit !found }
    '
}

[[ $# -ge 1 ]] || { usage >&2; exit 2; }
action="$1"
shift
config=''
if [[ "${1:-}" == --config ]]; then
  [[ $# -eq 2 ]] || { usage >&2; exit 2; }
  config="$2"
  shift 2
fi
[[ $# -eq 0 ]] || { usage >&2; exit 2; }
case "${action}" in
  install | remove | status) ;;
  -h | --help | help) usage; exit 0 ;;
  *) usage >&2; exit 2 ;;
esac

((EUID == 0)) || fail 'run this reviewed helper through sudo'
command -v ufw >/dev/null 2>&1 || fail 'ufw is not installed'
[[ -n "${config}" ]] || config="$(default_config)"
load_config "${config}"

ufw_state="$(ufw status 2>/dev/null | sed -n '1s/^Status: //p')"
if [[ "${ufw_state}" != active ]]; then
  [[ "${action}" == status ]] && printf 'inactive\n' ||
    printf 'UFW is inactive; no firewall state was changed.\n'
  exit 0
fi

case "${action}" in
  install)
    if ! rule_present; then
      ufw allow proto tcp from "${JBL_IP}" to any port "${JBL_GENA_CALLBACK_PORT}" \
        comment "${RULE_COMMENT}" >/dev/null
    fi
    rule_present || fail 'the narrow callback rule was not installed'
    printf 'installed\n'
    ;;
  remove)
    if rule_present; then
      ufw --force delete allow proto tcp from "${JBL_IP}" \
        to any port "${JBL_GENA_CALLBACK_PORT}" >/dev/null
    fi
    ! rule_present || fail 'the callback rule is still present'
    printf 'removed\n'
    ;;
  status)
    if rule_present; then
      printf 'installed\n'
    else
      printf 'missing\n'
      exit 1
    fi
    ;;
esac
