#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
rust_dir="${repo_dir}/rust"
output_dir="${repo_dir}/sbom"
notice_output="${repo_dir}/THIRD_PARTY_LICENSES.md"
spdx_output="${output_dir}/jbl-aura-link.spdx-2.3.json"
cyclonedx_output="${output_dir}/jbl-aura-link.cyclonedx-1.6.json"

for command_name in awk cargo cargo-about cargo-sbom jq rg sha256sum install; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    printf 'release metadata: required tool is unavailable\n' >&2
    exit 1
  }
done

[[ "$(cargo about --version)" == 'cargo-about 0.9.2' ]] || {
  printf 'release metadata: cargo-about must be version 0.9.2\n' >&2
  exit 1
}
[[ "$(cargo sbom --version)" == 'cargo-sbom 0.10.0' ]] || {
  printf 'release metadata: cargo-sbom must be version 0.10.0\n' >&2
  exit 1
}

lock_before="$(sha256sum "${rust_dir}/Cargo.lock" | cut -d ' ' -f 1)"
temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/jbl-aura-release-metadata.XXXXXX")"
public_dir="${temp_dir}/public"
cleanup() {
  case "${temp_dir}" in
    "${TMPDIR:-/tmp}"/jbl-aura-release-metadata.*)
      rm -rf -- "${temp_dir}"
      ;;
  esac
}
trap cleanup EXIT
mkdir -p "${public_dir}"

(
  cd "${rust_dir}"
  CARGO_NET_OFFLINE=true cargo metadata --locked --offline --format-version 1 \
    >"${temp_dir}/cargo-metadata.json"
  CARGO_NET_OFFLINE=true cargo about generate \
    --config about.toml \
    --frozen \
    --fail \
    --output-file "${public_dir}/THIRD_PARTY_LICENSES.md" \
    third_party_licenses.hbs
  # Some upstream license texts use CRLF and cargo-about preserves those
  # bytes inside the generated Markdown. Normalize only line endings and
  # terminal blank lines so `git diff --check` remains a useful release gate;
  # license wording and interior whitespace are left unchanged.
  LC_ALL=C awk '
    { sub(/\r$/, ""); lines[NR] = $0; if ($0 !~ /^[[:space:]]*$/) last = NR }
    END { for (line = 1; line <= last; line++) print lines[line] }
  ' "${public_dir}/THIRD_PARTY_LICENSES.md" \
    >"${temp_dir}/THIRD_PARTY_LICENSES.normalized.md"
  install -m 0644 "${temp_dir}/THIRD_PARTY_LICENSES.normalized.md" \
    "${public_dir}/THIRD_PARTY_LICENSES.md"
  CARGO_NET_OFFLINE=true cargo sbom \
    --cargo-package jbl-aura-link \
    --project-directory . \
    --output-format spdx_json_2_3 \
    >"${public_dir}/jbl-aura-link.spdx-2.3.json"
  CARGO_NET_OFFLINE=true cargo sbom \
    --cargo-package jbl-aura-link \
    --project-directory . \
    --output-format cyclone_dx_json_1_6 \
    >"${public_dir}/jbl-aura-link.cyclonedx-1.6.json"
)

lock_after="$(sha256sum "${rust_dir}/Cargo.lock" | cut -d ' ' -f 1)"
[[ "${lock_before}" == "${lock_after}" ]] || {
  printf 'release metadata: Cargo.lock changed during generation\n' >&2
  exit 1
}

jq -e \
  '.spdxVersion == "SPDX-2.3" and .dataLicense == "CC0-1.0" and (.packages | length > 1)' \
  "${public_dir}/jbl-aura-link.spdx-2.3.json" >/dev/null || {
  printf 'release metadata: SPDX validation failed\n' >&2
  exit 1
}
jq -e \
  '.bomFormat == "CycloneDX" and .specVersion == "1.6" and (.components | length > 0)' \
  "${public_dir}/jbl-aura-link.cyclonedx-1.6.json" >/dev/null || {
  printf 'release metadata: CycloneDX validation failed\n' >&2
  exit 1
}

# Do not publish environment paths, LAN identities, device addresses, or
# credential-shaped material through generated release metadata.  The full
# repository privacy gate runs after installation as an additional check.
home_word='home'
users_word='Users'
private_material_pattern="(/${home_word}/|/${users_word}/|[A-Za-z]:\\\\${users_word}\\\\|(^|[^0-9])(10\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}|192\\.168\\.[0-9]{1,3}\\.[0-9]{1,3}|172\\.(1[6-9]|2[0-9]|3[01])\\.[0-9]{1,3}\\.[0-9]{1,3})([^0-9]|$)|([[:xdigit:]]{2}:){5}[[:xdigit:]]{2}|gh[pousr]_[A-Za-z0-9_]{20,}|BEGIN ((RSA|EC|DSA|OPENSSH|ENCRYPTED) )?PRIVATE KEY)"
if rg -a -q --no-messages -e "${private_material_pattern}" -- \
  "${public_dir}/THIRD_PARTY_LICENSES.md" \
  "${public_dir}/jbl-aura-link.spdx-2.3.json" \
  "${public_dir}/jbl-aura-link.cyclonedx-1.6.json"; then
  printf 'release metadata: generated output failed privacy validation\n' >&2
  exit 1
fi

# Run the complete public-tree rules before generated data can replace files in
# the checkout.  Alternate roots are enabled only for this bounded self-check.
PRIVACY_SELF_TEST=1 PRIVACY_SCAN_ROOT="${public_dir}" \
  PYTHON_BIN="${PYTHON_BIN:-python3}" \
  "${repo_dir}/tests/privacy.sh" >/dev/null

mkdir -p "${output_dir}"
install -m 0644 "${public_dir}/THIRD_PARTY_LICENSES.md" "${notice_output}"
install -m 0644 "${public_dir}/jbl-aura-link.spdx-2.3.json" "${spdx_output}"
install -m 0644 "${public_dir}/jbl-aura-link.cyclonedx-1.6.json" "${cyclonedx_output}"

PYTHON_BIN="${PYTHON_BIN:-python3}" "${repo_dir}/tests/privacy.sh" >/dev/null
printf 'release metadata: PASS\n'
