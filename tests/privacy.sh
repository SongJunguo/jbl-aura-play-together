#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -n "${PRIVACY_SCAN_ROOT:-}" && "${PRIVACY_SELF_TEST:-0}" != 1 ]]; then
  printf 'privacy check: alternate scan roots are self-test only\n' >&2
  exit 1
fi
scan_root="${PRIVACY_SCAN_ROOT:-${repo_dir}}"
[[ -d "${scan_root}" ]] || {
  printf 'privacy check: scan root is not a directory\n' >&2
  exit 1
}
cd "${scan_root}"

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  mapfile -d '' files < <(git ls-files --cached --others --exclude-standard -z)
else
  mapfile -d '' files < <(
    find -P . \( -type f -o -type l \) \
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

# Do this before `rg`, Python, extension inspection, or any other content
# access.  An explicitly named symlink can otherwise cause a scanner to read a
# target outside the checkout.  The diagnostic intentionally contains neither
# the link name nor its target because either can itself contain private data.
symlink_candidates=0
for candidate in "${files[@]}"; do
  if [[ -L "${candidate}" ]]; then
    symlink_candidates=$((symlink_candidates + 1))
  fi
done
if ((symlink_candidates != 0)); then
  printf 'privacy check: symbolic-link candidate refused (hits=%s; details redacted)\n' \
    "${symlink_candidates}" >&2
  exit 1
fi

scan() {
  local pattern="$1"
  rg -a -n --color never --no-heading -e "${pattern}" -- "${files[@]}" 2>/dev/null || true
}

scan_history() {
  local pattern="$1" revision
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  while IFS= read -r revision; do
    git grep -n -I -E -e "${pattern}" "${revision}" -- . 2>/dev/null || true
  done < <(git rev-list --all)
}

report_hits() {
  local label="$1" hits="$2" count
  count="$(wc -l <<<"${hits}" | tr -d '[:space:]')"
  # Never echo the matching line or filename. A secret or device identifier
  # can itself appear in a filename, and CI logs are not a safe redaction
  # boundary. Developers can reproduce locally, then inspect the candidate
  # tree without publishing the value.
  printf 'privacy check: %s (hits=%s; details redacted)\n' \
    "${label}" "${count}" >&2
}

failed=0

mac_pattern='([[:xdigit:]]{2}:){5}[[:xdigit:]]{2}'
hyphen_mac_pattern='([[:xdigit:]]{2}-){5}[[:xdigit:]]{2}'
mac_hits="$(scan "(${mac_pattern}|${hyphen_mac_pattern})" |
  sed -e 's/02:00:00:00:00:01/<placeholder>/g' \
    -e 's/02:00:00:00:00:02/<placeholder>/g' \
    -e 's/02:00:00:00:00:03/<placeholder>/g' |
  rg -e "(${mac_pattern}|${hyphen_mac_pattern})" || true)"
if [[ -n "${mac_hits}" ]]; then
  report_hits 'non-placeholder Bluetooth address found' "${mac_hits}"
  failed=1
fi

# A PL/JSON payload can contain a display-form MAC encoded as ASCII hex, for
# example 30 32 3a ... for a placeholder beginning with 02:. Decode those
# candidate strings before applying the same placeholder allowlist. This
# catches the leak class that a plain colon-form regex misses.
encoded_mac_pattern='([[:xdigit:]]{4}3[aA]){5}[[:xdigit:]]{4}'
encoded_mac_hits="$(scan "${encoded_mac_pattern}" |
  rg -o -e "${encoded_mac_pattern}" |
  while IFS= read -r encoded; do
    decoded="$(printf '%s' "${encoded}" | xxd -r -p 2>/dev/null || true)"
    case "${decoded}" in
      02:00:00:00:00:01 | 02:00:00:00:00:02 | 02:00:00:00:00:03) ;;
      *) printf 'encoded-address\n' ;;
    esac
  done || true)"
if [[ -n "${encoded_mac_hits}" ]]; then
  report_hits 'non-placeholder hex-encoded Bluetooth address found' "${encoded_mac_hits}"
  failed=1
fi

home_word='home'
users_word='Users'
linux_home_pattern="/${home_word}/"
mac_home_pattern="/${users_word}/"
windows_home_pattern="[A-Za-z]:\\\\${users_word}\\\\"
path_hits="$(scan "(${linux_home_pattern}|${mac_home_pattern}|${windows_home_pattern})" || true)"
if [[ -n "${path_hits}" ]]; then
  report_hits 'absolute user path found' "${path_hits}"
  failed=1
fi

private_ip_pattern='(^|[^0-9])(10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3}|172\.(1[6-9]|2[0-9]|3[01])\.[0-9]{1,3}\.[0-9]{1,3})([^0-9]|$)'
private_ipv6_pattern='(^|[^[:xdigit:]])([fF][cCdD][[:xdigit:]]{2}|[fF][eE][89aAbB][[:xdigit:]]):([[:xdigit:]]{0,4}:){1,}[[:xdigit:]]{0,4}([^[:xdigit:]]|$)'
private_ip_hits="$(scan "${private_ip_pattern}" || true)"
if [[ -n "${private_ip_hits}" ]]; then
  report_hits 'private network address found' "${private_ip_hits}"
  failed=1
fi
private_ipv6_hits="$(scan "${private_ipv6_pattern}" || true)"
if [[ -n "${private_ipv6_hits}" ]]; then
  report_hits 'private or link-local IPv6 address found' "${private_ipv6_hits}"
  failed=1
fi

begin_word='BEGIN'
secret_pattern="(${begin_word} ((RSA|EC|DSA|OPENSSH|ENCRYPTED) )?PRIVATE KEY|${begin_word} PGP PRIVATE KEY BLOCK|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|xox[baprs]-[A-Za-z0-9-]{10,}|glpat-[A-Za-z0-9_-]{20,}|sk_(live|test)_[A-Za-z0-9]{20,}|sk-(proj-)?[A-Za-z0-9_-]{20,}|eyJ[A-Za-z0-9_-]{8,}[.][A-Za-z0-9_-]{8,}[.][A-Za-z0-9_-]{8,})"
secret_hits="$(scan "${secret_pattern}" || true)"
if [[ -n "${secret_hits}" ]]; then
  report_hits 'credential-shaped material found' "${secret_hits}"
  failed=1
fi

tls_pin_pattern='JBL_LOCAL_API_TLS_SHA256[[:space:]]*=[[:space:]]*[[:xdigit:]]{64}'
tls_pin_placeholder_pattern='JBL_LOCAL_API_TLS_SHA256[[:space:]]*=[[:space:]]*0{64}$'
tls_pin_hits="$(scan "${tls_pin_pattern}" |
  rg -o -e "${tls_pin_pattern}" |
  rg -v -e "${tls_pin_placeholder_pattern}" || true)"
if [[ -n "${tls_pin_hits}" ]]; then
  report_hits 'device TLS fingerprint found' "${tls_pin_hits}"
  failed=1
fi

certificate_word='CERTIFICATE'
certificate_pattern="${begin_word} ((X509|TRUSTED) )?${certificate_word}|${begin_word} ${certificate_word} REQUEST|${begin_word} (PKCS7|CMS)"
certificate_hits="$(scan "${certificate_pattern}" || true)"
if [[ -n "${certificate_hits}" ]]; then
  report_hits 'certificate material found' "${certificate_hits}"
  failed=1
fi

# A PEM boundary is not required for a leaked body: files copied from a secret
# store sometimes contain only the Base64 lines. DER X.509/PKCS structures begin
# with an ASN.1 SEQUENCE and therefore normally encode with `M`; OpenSSH's
# private-key envelope has a stable Base64 prefix. Keep the threshold at one
# wrapped PEM line so a headerless, line-wrapped key is still rejected.
headerless_key_pattern='(^[[:space:]]*M[A-Za-z0-9+/]{39,}={0,2}[[:space:]]*$|b3BlbnNzaC1rZXktdjE[A-Za-z0-9+/=]{20,})'
headerless_key_hits="$(scan "${headerless_key_pattern}" || true)"
if [[ -n "${headerless_key_hits}" ]]; then
  report_hits 'headerless PEM/DER/OpenSSH body found' "${headerless_key_hits}"
  failed=1
fi

# Current-tree scans cannot detect a secret removed by a later commit. Check
# every reachable revision for the highest-risk credential and address shapes.
history_secret_hits="$(scan_history "${secret_pattern}" || true)"
if [[ -n "${history_secret_hits}" ]]; then
  report_hits 'credential-shaped material found in Git history' "${history_secret_hits}"
  failed=1
fi

history_tls_pin_hits="$(scan_history "${tls_pin_pattern}" |
  rg -o -e "${tls_pin_pattern}" |
  rg -v -e "${tls_pin_placeholder_pattern}" || true)"
if [[ -n "${history_tls_pin_hits}" ]]; then
  report_hits 'device TLS fingerprint found in Git history' "${history_tls_pin_hits}"
  failed=1
fi

history_mac_hits="$(scan_history "(${mac_pattern}|${hyphen_mac_pattern})" |
  sed -e 's/02:00:00:00:00:01/<placeholder>/g' \
    -e 's/02:00:00:00:00:02/<placeholder>/g' \
    -e 's/02:00:00:00:00:03/<placeholder>/g' |
  rg -e "(${mac_pattern}|${hyphen_mac_pattern})" || true)"
if [[ -n "${history_mac_hits}" ]]; then
  report_hits 'non-placeholder Bluetooth address found in Git history' "${history_mac_hits}"
  failed=1
fi

history_encoded_mac_hits="$(scan_history "${encoded_mac_pattern}" |
  rg -o -e "${encoded_mac_pattern}" |
  while IFS= read -r encoded; do
    decoded="$(printf '%s' "${encoded}" | xxd -r -p 2>/dev/null || true)"
    case "${decoded}" in
      02:00:00:00:00:01 | 02:00:00:00:00:02 | 02:00:00:00:00:03) ;;
      *) printf 'historical-encoded-address\n' ;;
    esac
  done || true)"
if [[ -n "${history_encoded_mac_hits}" ]]; then
  report_hits 'non-placeholder hex-encoded Bluetooth address found in Git history' \
    "${history_encoded_mac_hits}"
  failed=1
fi

history_ip_hits="$(scan_history "${private_ip_pattern}" || true)"
if [[ -n "${history_ip_hits}" ]]; then
  report_hits 'private network address found in Git history' "${history_ip_hits}"
  failed=1
fi
history_ipv6_hits="$(scan_history "${private_ipv6_pattern}" || true)"
if [[ -n "${history_ipv6_hits}" ]]; then
  report_hits 'private or link-local IPv6 address found in Git history' \
    "${history_ipv6_hits}"
  failed=1
fi

history_path_hits="$(scan_history "(${linux_home_pattern}|${mac_home_pattern}|${windows_home_pattern})" || true)"
if [[ -n "${history_path_hits}" ]]; then
  report_hits 'absolute user path found in Git history' "${history_path_hits}"
  failed=1
fi

history_certificate_hits="$(scan_history "${certificate_pattern}" || true)"
if [[ -n "${history_certificate_hits}" ]]; then
  report_hits 'certificate material found in Git history' "${history_certificate_hits}"
  failed=1
fi

history_headerless_key_hits="$(scan_history "${headerless_key_pattern}" || true)"
if [[ -n "${history_headerless_key_hits}" ]]; then
  report_hits 'headerless PEM/DER/OpenSSH body found in Git history' \
    "${history_headerless_key_hits}"
  failed=1
fi

disallowed_files=''
for candidate in "${files[@]}"; do
  case "${candidate,,}" in
    *.pem | *.key | *.crt | *.cer | *.der | *.p7b | *.p8 | *.p12 | *.pfx | \
      *.pkcs8 | *.pkcs12 | *.ppk | *.jks | *.keystore | *.kdbx | \
      *.mobileprovision | *.ipa | *.apk | *.aab | *.apks | *.xapk | *.dex | \
      *.pcap | *.pcapng | *.cap | *.hccapx | *.btsnoop | *.btsnoop_hci | \
      *.har | *.saz | *.mitm | *.mitmweb | *.ab | *.bin | *.img | *.firmware | *.fw)
      disallowed_files+="${candidate}:disallowed-public-file"$'\n'
      ;;
  esac
done
if [[ -n "${disallowed_files}" ]]; then
  report_hits 'disallowed credential/capture/binary file found' "${disallowed_files%$'\n'}"
  failed=1
fi

python_bin="${PYTHON_BIN:-python3}"
command -v "${python_bin}" >/dev/null 2>&1 || {
  printf 'privacy check: configured Python is required for binary-file inspection\n' >&2
  exit 1
}
binary_files="$(printf '%s\0' "${files[@]}" | "${python_bin}" -c '
import pathlib, sys
for raw in sys.stdin.buffer.read().split(b"\0"):
    if not raw:
        continue
    path = pathlib.Path(raw.decode("utf-8", "surrogateescape"))
    try:
        payload = path.read_bytes()
    except OSError:
        print(f"{path}:unreadable")
        continue
    if b"\0" in payload:
        print(f"{path}:binary-content")
')"
if [[ -n "${binary_files}" ]]; then
  report_hits 'tracked/unignored binary content found' "${binary_files}"
  failed=1
fi

history_file_hits=''
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  while IFS= read -r revision; do
    while IFS= read -r -d '' candidate; do
      case "${candidate,,}" in
        *.pem | *.key | *.crt | *.cer | *.der | *.p7b | *.p8 | *.p12 | *.pfx | \
          *.pkcs8 | *.pkcs12 | *.ppk | *.jks | *.keystore | *.kdbx | \
          *.mobileprovision | *.ipa | *.apk | *.aab | *.apks | *.xapk | *.dex | \
          *.pcap | *.pcapng | *.cap | *.hccapx | *.btsnoop | *.btsnoop_hci | \
          *.har | *.saz | *.mitm | *.mitmweb | *.ab | *.bin | *.img | \
          *.firmware | *.fw)
          history_file_hits+="${candidate}:historical-disallowed-file"$'\n'
          ;;
      esac
    done < <(git ls-tree -rz --name-only "${revision}")
  done < <(git rev-list --all)
fi
if [[ -n "${history_file_hits}" ]]; then
  report_hits 'disallowed file found in Git history' "${history_file_hits%$'\n'}"
  failed=1
fi

# `git grep -I` intentionally skips binary blobs, and a credential/capture can
# use an innocent extension such as .dat. Scan every unique reachable blob by
# object ID, without exposing its hash, historical filename, or contents.
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  history_blob_scanner="${repo_dir}/tests/history_blob_privacy.py"
  [[ -f "${history_blob_scanner}" ]] || {
    printf 'privacy check: history blob scanner is unavailable\n' >&2
    exit 1
  }
  if ! history_blob_output="$("${python_bin}" "${history_blob_scanner}" 2>&1)"; then
    printf '%s\n' "${history_blob_output}" >&2
    failed=1
  fi
fi

if ((failed != 0)); then
  exit 1
fi

printf 'privacy check: PASS\n'
