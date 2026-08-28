#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_dir}"

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  mapfile -d '' files < <(git ls-files --cached --others --exclude-standard -z)
else
  mapfile -d '' files < <(
    find . -type f \
      ! -path './.git/*' \
      ! -path './config/devices.env' \
      ! -path './state/*' \
      ! -path './runtime/*' \
      ! -path './logs/*' \
      ! -path './captures/*' \
      -print0
  )
fi

((${#files[@]} > 0)) || {
  printf 'privacy check: no candidate files found\n' >&2
  exit 1
}

scan() {
  local pattern="$1"
  rg -n --color never --no-heading -e "${pattern}" -- "${files[@]}" 2>/dev/null || true
}

failed=0

mac_hits="$(scan '([[:xdigit:]]{2}:){5}[[:xdigit:]]{2}' |
  sed -e 's/02:00:00:00:00:01/<placeholder>/g' \
    -e 's/02:00:00:00:00:02/<placeholder>/g' |
  rg -e '([[:xdigit:]]{2}:){5}[[:xdigit:]]{2}' || true)"
if [[ -n "${mac_hits}" ]]; then
  printf 'privacy check: non-placeholder Bluetooth address found:\n%s\n' "${mac_hits}" >&2
  failed=1
fi

home_word='home'
users_word='Users'
linux_home_pattern="/${home_word}/"
mac_home_pattern="/${users_word}/"
windows_home_pattern="[A-Za-z]:\\\\${users_word}\\\\"
path_hits="$(scan "(${linux_home_pattern}|${mac_home_pattern}|${windows_home_pattern})" || true)"
if [[ -n "${path_hits}" ]]; then
  printf 'privacy check: absolute user path found:\n%s\n' "${path_hits}" >&2
  failed=1
fi

private_ip_hits="$(scan '(^|[^0-9])(10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3}|172\.(1[6-9]|2[0-9]|3[01])\.[0-9]{1,3}\.[0-9]{1,3})([^0-9]|$)' || true)"
if [[ -n "${private_ip_hits}" ]]; then
  printf 'privacy check: private network address found:\n%s\n' "${private_ip_hits}" >&2
  failed=1
fi

secret_hits="$(scan '(BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16})' || true)"
if [[ -n "${secret_hits}" ]]; then
  printf 'privacy check: credential-shaped material found:\n%s\n' "${secret_hits}" >&2
  failed=1
fi

if ((failed != 0)); then
  exit 1
fi

printf 'privacy check: PASS\n'
