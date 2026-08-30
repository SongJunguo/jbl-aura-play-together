#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
privacy_check="${repo_dir}/tests/privacy.sh"
artifact_check="${repo_dir}/tests/artifact_strings_privacy.sh"
python_bin="${PYTHON_BIN:-python3}"

for command_name in git rg xxd "${python_bin}"; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    printf 'privacy self-test: missing required command\n' >&2
    exit 1
  }
done

test_root="$(mktemp -d "${TMPDIR:-/tmp}/jbl-aura-privacy-self-test.XXXXXX")"
cleanup() {
  case "${test_root}" in
    "${TMPDIR:-/tmp}"/jbl-aura-privacy-self-test.*)
      rm -rf -- "${test_root}"
      ;;
  esac
}
trap cleanup EXIT

repeat_char() {
  local character="$1" count="$2" output=''
  while ((count > 0)); do
    output+="${character}"
    count=$((count - 1))
  done
  printf '%s' "${output}"
}

mkdir -p "${test_root}/safe-source" "${test_root}/unsafe-source"

numeric_table=''
for value in {0..99}; do
  printf -v numeric_table '%s%02d' "${numeric_table}" "${value}"
done
printf 'generated_numeric_table=%s\nplaceholder=02:00:00:00:00:01\n' \
  "${numeric_table}" >"${test_root}/safe-source/safe.txt"
printf '%s\n' \
  '65786365-6c70-6f69-6e74-2e636f6d0000' \
  '65786365-6c70-6f69-6e74-2e636f6d0001' \
  '65786365-6c70-6f69-6e74-2e636f6d0002' \
  >>"${test_root}/safe-source/safe.txt"
printf 'JBL_LOCAL_API_TLS_SHA256=%s\n' "$(repeat_char 0 64)" \
  >>"${test_root}/safe-source/safe.txt"

safe_log="${test_root}/safe-source.log"
if ! PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${test_root}/safe-source" \
  PYTHON_BIN="${python_bin}" \
  "${privacy_check}" >"${safe_log}" 2>&1; then
  printf 'privacy self-test: safe source fixture was rejected\n' >&2
  exit 1
fi

token_prefix="$(printf '%s%s_' gh p)"
token_value="${token_prefix}$(repeat_char A 24)"
google_prefix="$(printf '%s%s' AI za)"
google_token="${google_prefix}$(repeat_char B 35)"
jwt_prefix="$(printf '%s%s' ey J)"
jwt_value="${jwt_prefix}$(repeat_char C 10).${jwt_prefix}$(repeat_char D 10).$(repeat_char E 12)"
begin_piece="$(printf '%s%s' BE GIN)"
private_marker="$(printf '%s %s %s' "${begin_piece}" PRIVATE KEY)"
certificate_marker="$(printf '%s %s' "${begin_piece}" CERTIFICATE)"
printf -v private_address '%s.%s.%s.%s' 192 168 44 7
printf -v bluetooth_address '%s:%s:%s:%s:%s:%s' A4 B5 C6 D7 E8 F9
printf -v hyphen_address '%s-%s-%s-%s-%s-%s' A4 B5 C6 D7 E8 F9
printf -v private_path '/%s/%s/%s' home synthetic secret.txt
printf -v private_ipv6 '%s%s::%s' fd 00 1
tls_pin="JBL_LOCAL_API_TLS_SHA256=$(repeat_char A 64)"
encoded_placeholder="$(printf '%s' '02:00:00:00:00:01' | xxd -p -c 256)"
encoded_private="$(printf '%s' "${bluetooth_address}" | xxd -p -c 256)"
headerless_der_body="M$(repeat_char F 63)"
openssh_body_prefix="$(printf '%s%s' b3BlbnNzaC1rZXkt djE)"
headerless_openssh_body="${openssh_body_prefix}$(repeat_char G 44)"

printf '%s\n' \
  "${token_value}" "${google_token}" "${jwt_value}" \
  "${private_marker}" "${certificate_marker}" "${private_address}" \
  "${private_ipv6}" "${bluetooth_address}" "${hyphen_address}" \
  "${private_path}" "${tls_pin}" "${headerless_der_body}" \
  "${headerless_openssh_body}" \
  "${encoded_placeholder}" "${encoded_private}" \
  >"${test_root}/unsafe-source/sensitive.txt"
printf 'synthetic credential container\n' \
  >"${test_root}/unsafe-source/credential.p12"
printf 'text-before-nul\0text-after-nul\n' \
  >"${test_root}/unsafe-source/binary.dat"

unsafe_log="${test_root}/unsafe-source.log"
if PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${test_root}/unsafe-source" \
  PYTHON_BIN="${python_bin}" \
  "${privacy_check}" >"${unsafe_log}" 2>&1; then
  printf 'privacy self-test: unsafe source fixture was accepted\n' >&2
  exit 1
fi
for sensitive_value in \
  "${token_value}" "${google_token}" "${jwt_value}" \
  "${private_marker}" "${certificate_marker}" "${private_address}" \
  "${private_ipv6}" "${bluetooth_address}" "${hyphen_address}" \
  "${private_path}" "${tls_pin}" "${encoded_private}" \
  "${headerless_der_body}" "${headerless_openssh_body}"; do
  if rg -F -q -- "${sensitive_value}" "${unsafe_log}"; then
    printf 'privacy self-test: source failure log exposed fixture data\n' >&2
    exit 1
  fi
done
rg -q 'details redacted' "${unsafe_log}" || {
  printf 'privacy self-test: source failure was not explicitly redacted\n' >&2
  exit 1
}

# Each high-risk source rule must reject a fixture by itself. A single combined
# unsafe file is insufficient because one working regex could otherwise hide a
# regression in another rule.
source_case_index=0
expect_source_rejected() {
  local payload="$1" case_dir case_log
  source_case_index=$((source_case_index + 1))
  case_dir="${test_root}/source-case-${source_case_index}"
  case_log="${test_root}/source-case-${source_case_index}.log"
  mkdir -p "${case_dir}"
  printf '%s\n' "${payload}" >"${case_dir}/sensitive.txt"
  if PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${case_dir}" \
    PYTHON_BIN="${python_bin}" "${privacy_check}" >"${case_log}" 2>&1; then
    printf 'privacy self-test: isolated unsafe source fixture was accepted\n' >&2
    exit 1
  fi
  if rg -F -q -- "${payload}" "${case_log}"; then
    printf 'privacy self-test: isolated source log exposed fixture data\n' >&2
    exit 1
  fi
}

for isolated_value in \
  "${token_value}" "${google_token}" "${jwt_value}" \
  "${private_marker}" "${certificate_marker}" "${private_address}" \
  "${private_ipv6}" "${bluetooth_address}" "${hyphen_address}" \
  "${private_path}" "${tls_pin}" "${encoded_private}" \
  "${headerless_der_body}" "${headerless_openssh_body}"; do
  expect_source_rejected "${isolated_value}"
done

extension_case="${test_root}/source-case-extension"
mkdir -p "${extension_case}"
printf 'synthetic container\n' >"${extension_case}/credential.p12"
if PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${extension_case}" \
  PYTHON_BIN="${python_bin}" "${privacy_check}" >/dev/null 2>&1; then
  printf 'privacy self-test: isolated disallowed extension was accepted\n' >&2
  exit 1
fi

binary_case="${test_root}/source-case-binary"
mkdir -p "${binary_case}"
printf 'before\0after\n' >"${binary_case}/payload.dat"
if PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${binary_case}" \
  PYTHON_BIN="${python_bin}" "${privacy_check}" >/dev/null 2>&1; then
  printf 'privacy self-test: isolated binary content was accepted\n' >&2
  exit 1
fi

# A deleted binary blob with an innocent extension must still fail the full
# reachable-history scan. The old `git grep -I` plus extension checks missed
# exactly this case.
history_root="${test_root}/history-only"
mkdir -p "${history_root}"
git -C "${history_root}" init -q
git -C "${history_root}" config user.name 'Privacy Self Test'
git -C "${history_root}" config user.email 'privacy-self-test@example.invalid'
git -C "${history_root}" config commit.gpgsign false
printf 'binary-prefix\0%s\n' "${token_value}" \
  >"${history_root}/historical.dat"
git -C "${history_root}" add historical.dat
git -C "${history_root}" commit -q -m 'synthetic unsafe history'
unlink "${history_root}/historical.dat"
printf 'safe current tree\n' >"${history_root}/visible.txt"
git -C "${history_root}" add -A
git -C "${history_root}" commit -q -m 'remove synthetic history fixture'

history_log="${test_root}/history-only.log"
if PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${history_root}" \
  PYTHON_BIN="${python_bin}" "${privacy_check}" >"${history_log}" 2>&1; then
  printf 'privacy self-test: deleted binary history fixture was accepted\n' >&2
  exit 1
fi
rg -q 'history blob privacy: binary blob found' "${history_log}" || {
  printf 'privacy self-test: historical binary blob was not classified\n' >&2
  exit 1
}
rg -q 'history blob privacy: credential-shaped blob found' "${history_log}" || {
  printf 'privacy self-test: historical credential blob was not classified\n' >&2
  exit 1
}
if rg -F -q -- "${token_value}" "${history_log}" || \
  rg -F -q -- 'historical.dat' "${history_log}"; then
  printf 'privacy self-test: history failure log exposed fixture data\n' >&2
  exit 1
fi

safe_strings="${test_root}/safe-strings.txt"
printf '%s\n' \
  "${numeric_table}" \
  '/opt/jbl-aura-build/release/build/openssl/install/lib/ossl-modules' \
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789' \
  "${private_marker}" \
  >"${safe_strings}"
printf '%s\n' \
  '65786365-6c70-6f69-6e74-2e636f6d0000' \
  '65786365-6c70-6f69-6e74-2e636f6d0001' \
  '65786365-6c70-6f69-6e74-2e636f6d0002' \
  >>"${safe_strings}"
printf 'JBL_LOCAL_API_TLS_SHA256=%s\n' "$(repeat_char 0 64)" \
  >>"${safe_strings}"
if ! "${artifact_check}" "${safe_strings}" \
  >"${test_root}/safe-artifact.log" 2>&1; then
  printf 'privacy self-test: safe artifact strings were rejected\n' >&2
  exit 1
fi

pem_body="M$(repeat_char A 110)="
wrapped_pem_body="M$(repeat_char B 63)"
unsafe_strings="${test_root}/unsafe-strings.txt"
printf '%s\n' \
  "${pem_body}" "${token_value}" "${google_token}" "${jwt_value}" \
  "${private_address}" "${private_ipv6}" "${bluetooth_address}" \
  "${hyphen_address}" "${private_path}" "${tls_pin}" "${certificate_marker}" \
  "${wrapped_pem_body}" "${headerless_openssh_body}" >"${unsafe_strings}"
artifact_log="${test_root}/unsafe-artifact.log"
if "${artifact_check}" "${unsafe_strings}" >"${artifact_log}" 2>&1; then
  printf 'privacy self-test: unsafe artifact strings were accepted\n' >&2
  exit 1
fi
for sensitive_value in \
  "${pem_body}" "${token_value}" "${google_token}" "${jwt_value}" \
  "${private_address}" "${private_ipv6}" "${bluetooth_address}" \
  "${hyphen_address}" "${private_path}" "${tls_pin}" \
  "${wrapped_pem_body}" "${headerless_openssh_body}"; do
  if rg -F -q -- "${sensitive_value}" "${artifact_log}"; then
    printf 'privacy self-test: artifact failure log exposed fixture data\n' >&2
    exit 1
  fi
done
rg -q 'PEM/DER-like Base64 body found' "${artifact_log}" || {
  printf 'privacy self-test: PEM-like body was not classified\n' >&2
  exit 1
}
rg -q 'wrapped PEM body found' "${artifact_log}" || {
  printf 'privacy self-test: wrapped PEM body was not classified\n' >&2
  exit 1
}
rg -q 'OpenSSH private-key Base64 body found' "${artifact_log}" || {
  printf 'privacy self-test: OpenSSH body was not classified\n' >&2
  exit 1
}
rg -q 'details redacted' "${artifact_log}" || {
  printf 'privacy self-test: artifact failure was not explicitly redacted\n' >&2
  exit 1
}

# Exercise each artifact rule independently.  The combined fixture above checks
# aggregate redaction, but cannot prove that every individual high-risk rule is
# still active.
artifact_case_index=0
expect_artifact_rejected() {
  local payload="$1" case_file case_log sensitive_line
  artifact_case_index=$((artifact_case_index + 1))
  case_file="${test_root}/artifact-case-${artifact_case_index}.txt"
  case_log="${test_root}/artifact-case-${artifact_case_index}.log"
  printf '%s\n' "${payload}" >"${case_file}"
  if "${artifact_check}" "${case_file}" >"${case_log}" 2>&1; then
    printf 'privacy self-test: isolated unsafe artifact fixture was accepted\n' >&2
    exit 1
  fi
  while IFS= read -r sensitive_line; do
    [[ -n "${sensitive_line}" ]] || continue
    if rg -F -q -- "${sensitive_line}" "${case_log}"; then
      printf 'privacy self-test: isolated artifact log exposed fixture data\n' >&2
      exit 1
    fi
  done <<<"${payload}"
  rg -q 'details redacted' "${case_log}" || {
    printf 'privacy self-test: isolated artifact failure was not redacted\n' >&2
    exit 1
  }
}

for isolated_artifact_value in \
  "${private_path}" "${private_address}" "${private_ipv6}" \
  "${bluetooth_address}" "${hyphen_address}" \
  "${token_value}" "${google_token}" "${jwt_value}" \
  "${tls_pin}" "${headerless_der_body}" "${pem_body}" \
  "${headerless_openssh_body}"; do
  expect_artifact_rejected "${isolated_artifact_value}"
done
expect_artifact_rejected "${certificate_marker}"$'\n'"${wrapped_pem_body}"

# A current-tree symlink is rejected before the scanner can follow it.  Neither
# the candidate name nor the target is allowed into the failure log.
symlink_case="${test_root}/source-case-symlink"
symlink_target="${test_root}/private-symlink-target"
symlink_name="fixture-link-private-name"
symlink_secret="${token_prefix}$(repeat_char Z 24)"
mkdir -p "${symlink_case}"
printf '%s\n' "${symlink_secret}" >"${symlink_target}"
ln -s -- "${symlink_target}" "${symlink_case}/${symlink_name}"
symlink_log="${test_root}/source-case-symlink.log"
if PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${symlink_case}" \
  PYTHON_BIN="${python_bin}" "${privacy_check}" >"${symlink_log}" 2>&1; then
  printf 'privacy self-test: symlink source fixture was accepted\n' >&2
  exit 1
fi
rg -q 'symbolic-link candidate refused' "${symlink_log}" || {
  printf 'privacy self-test: symlink source fixture was not classified\n' >&2
  exit 1
}
for sensitive_value in "${symlink_name}" "${symlink_target}" "${symlink_secret}"; do
  if rg -F -q -- "${sensitive_value}" "${symlink_log}"; then
    printf 'privacy self-test: symlink failure log exposed fixture data\n' >&2
    exit 1
  fi
done

# Repeat through the Git candidate enumerator used by the real checkout, not
# only the non-Git `find` fallback above.
git -C "${symlink_case}" init -q
git -C "${symlink_case}" add -- "${symlink_name}"
tracked_symlink_log="${test_root}/source-case-tracked-symlink.log"
if PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${symlink_case}" \
  PYTHON_BIN="${python_bin}" "${privacy_check}" \
  >"${tracked_symlink_log}" 2>&1; then
  printf 'privacy self-test: tracked symlink fixture was accepted\n' >&2
  exit 1
fi
rg -q 'symbolic-link candidate refused' "${tracked_symlink_log}" || {
  printf 'privacy self-test: tracked symlink fixture was not classified\n' >&2
  exit 1
}
for sensitive_value in "${symlink_name}" "${symlink_target}" "${symlink_secret}"; do
  if rg -F -q -- "${sensitive_value}" "${tracked_symlink_log}"; then
    printf 'privacy self-test: tracked symlink log exposed fixture data\n' >&2
    exit 1
  fi
done

printf 'privacy self-test: PASS\n'
