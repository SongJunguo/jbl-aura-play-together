#!/usr/bin/env bash
set -euo pipefail

binary="${1:-rust/target/release/jbl-aura-link}"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
[[ -f "${binary}" && -x "${binary}" ]] || {
  printf 'release privacy: executable not found\n' >&2
  exit 1
}

for command_name in file ldd readelf strings; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    printf 'release privacy: missing command %s\n' "${command_name}" >&2
    exit 1
  }
done

file "${binary}" | rg -q 'ELF 64-bit.*x86-64' || {
  printf 'release privacy: unexpected Ubuntu artifact type\n' >&2
  exit 1
}

unexpected_libraries="$(ldd "${binary}" 2>/dev/null | awk '{print $1}' |
  rg -v '^(linux-vdso\.so\.1|libc\.so\.6|libgcc_s\.so\.1|/lib64/ld-linux-x86-64\.so\.2)$' || true)"
if [[ -n "${unexpected_libraries}" ]]; then
  printf 'release privacy: unexpected dynamic dependency names found (count=%s)\n' \
    "$(wc -l <<<"${unexpected_libraries}" | tr -d '[:space:]')" >&2
  exit 1
fi

max_glibc="$(readelf --version-info "${binary}" 2>/dev/null |
  rg -o 'GLIBC_[0-9]+([.][0-9]+)+' | sort -Vu | tail -n 1 | cut -d_ -f2)"
[[ -n "${max_glibc}" ]] || {
  printf 'release privacy: could not determine GLIBC requirement\n' >&2
  exit 1
}
if [[ "$(printf '%s\n%s\n' "2.35" "${max_glibc}" | sort -V | tail -n 1)" != "2.35" ]]; then
  printf 'release privacy: GLIBC requirement exceeds Ubuntu 22.04 (max=%s)\n' \
    "${max_glibc}" >&2
  exit 1
fi

string_dump="$(mktemp)"
trap 'rm -f -- "${string_dump}"' EXIT
strings -a "${binary}" >"${string_dump}"

"${script_dir}/artifact_strings_privacy.sh" "${string_dump}"
printf 'release privacy: PASS bytes=%s max_glibc=%s\n' \
  "$(stat -c '%s' "${binary}")" "${max_glibc}"
