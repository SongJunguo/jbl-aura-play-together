#!/usr/bin/env bash
set -euo pipefail

string_dump="${1:-}"
[[ -f "${string_dump}" ]] || {
  printf 'artifact string privacy: input not found\n' >&2
  exit 1
}

failed=0

scan_count() {
  local label="$1" pattern="$2" count
  count="$(LC_ALL=C rg -c --no-messages -e "${pattern}" -- "${string_dump}" || true)"
  count="${count:-0}"
  if [[ "${count}" != 0 ]]; then
    # Deliberately omit the value and input path. Both can contain the secret
    # this check is meant to keep out of CI and release logs.
    printf 'artifact string privacy: %s (hits=%s; details redacted)\n' \
      "${label}" "${count}" >&2
    failed=1
  fi
}

scan_match_count() {
  local label="$1" pattern="$2" count
  count="$(
    { LC_ALL=C rg -o --no-messages -e "${pattern}" -- "${string_dump}" || true; } |
      wc -l | tr -d '[:space:]'
  )"
  count="${count:-0}"
  if [[ "${count}" != 0 ]]; then
    printf 'artifact string privacy: %s (hits=%s; details redacted)\n' \
      "${label}" "${count}" >&2
    failed=1
  fi
}

scan_tls_pin_count() {
  local pin_pattern placeholder_pattern count
  pin_pattern='JBL_LOCAL_API_TLS_SHA256[[:space:]]*=[[:space:]]*[[:xdigit:]]{64}'
  placeholder_pattern='JBL_LOCAL_API_TLS_SHA256[[:space:]]*=[[:space:]]*0{64}$'
  count="$(
    { LC_ALL=C rg -o --no-messages -e "${pin_pattern}" -- "${string_dump}" || true; } |
      { LC_ALL=C rg -v -e "${placeholder_pattern}" || true; } |
      wc -l | tr -d '[:space:]'
  )"
  count="${count:-0}"
  if [[ "${count}" != 0 ]]; then
    printf 'artifact string privacy: device TLS fingerprint found (hits=%s; details redacted)\n' \
      "${count}" >&2
    failed=1
  fi
}

scan_wrapped_pem_count() {
  local count
  count="$(LC_ALL=C awk \
    -v begin_word="${begin_word}" \
    -v private_word="${private_word}" \
    -v certificate_word="${certificate_word}" '
      function is_boundary(line) {
        return line ~ (begin_word " .*" private_word " KEY") ||
          line ~ (begin_word " .*" certificate_word)
      }
      function is_der_body(line) {
        return length(line) >= 40 && line ~ /^M[A-Za-z0-9+\/=]+$/
      }
      {
        if (window > 0) {
          if (is_der_body($0)) {
            matches += 1
            window = 0
          } else {
            window -= 1
          }
        }
        if (is_boundary($0)) {
          window = 6
        }
      }
      END { print matches + 0 }
    ' "${string_dump}")"
  if [[ "${count}" != 0 ]]; then
    printf 'artifact string privacy: wrapped PEM body found (hits=%s; details redacted)\n' \
      "${count}" >&2
    failed=1
  fi
}

home_word='home'
users_word='Users'
scan_count 'environment-specific user/build path found' \
  "(/${home_word}/|/root/|/${users_word}/|/private/tmp/|/tmp/|/var/tmp/|/run/user/[0-9]+/|/github/workspace/|/__w/|[A-Za-z]:\\\\${users_word}\\\\)"

scan_count 'private IPv4 address found' \
  '(^|[^0-9])(10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3}|172\.(1[6-9]|2[0-9]|3[01])\.[0-9]{1,3}\.[0-9]{1,3})([^0-9]|$)'
scan_count 'private or link-local IPv6 address found' \
  '(^|[^[:xdigit:]])([fF][cCdD][[:xdigit:]]{2}|[fF][eE][89aAbB][[:xdigit:]]):([[:xdigit:]]{0,4}:){1,}[[:xdigit:]]{0,4}([^[:xdigit:]]|$)'

scan_count 'Bluetooth-address-shaped value found' \
  '(([[:xdigit:]]{2}:){5}[[:xdigit:]]{2}|([[:xdigit:]]{2}-){5}[[:xdigit:]]{2})'

scan_count 'credential-shaped token found' \
  '(gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|xox[baprs]-[A-Za-z0-9-]{10,}|glpat-[A-Za-z0-9_-]{20,}|sk_(live|test)_[A-Za-z0-9]{20,}|sk-(proj-)?[A-Za-z0-9_-]{20,}|eyJ[A-Za-z0-9_-]{8,}[.][A-Za-z0-9_-]{8,}[.][A-Za-z0-9_-]{8,})'
scan_tls_pin_count

begin_word='BEGIN'
private_word='PRIVATE'
certificate_word='CERTIFICATE'
scan_wrapped_pem_count

# PEM-encoded X.509 certificates and PKCS private keys are DER SEQUENCE values,
# whose unwrapped Base64 body normally begins with "M". Restricting the check
# to that shape avoids treating ICU's generated 00..99 table (one 200-digit
# printable string) as a secret while still detecting a headerless, unwrapped
# PEM/DER body. A delimiter alone is not secret because the executable needs
# delimiter constants to parse runtime credentials; a delimiter followed by a
# DER-shaped body is rejected by the wrapped-body check above.
scan_match_count 'PEM/DER-like Base64 body found' \
  '(^|[^A-Za-z0-9+/])M[A-Za-z0-9+/]{39,}={0,2}([^A-Za-z0-9+/=]|$)'
scan_match_count 'OpenSSH private-key Base64 body found' \
  'b3BlbnNzaC1rZXktdjE[A-Za-z0-9+/=]{20,}'

((failed == 0)) || exit 1
printf 'artifact string privacy: PASS\n'
