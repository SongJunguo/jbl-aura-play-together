#!/usr/bin/env bash
set -euo pipefail

rust_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "${rust_dir}/.." && pwd)"
output_binary="${1:-${rust_dir}/target/neutral/jbl-aura-link}"

for command_name in bwrap getent install mktemp realpath; do
  command -v "${command_name}" >/dev/null 2>&1 || {
    printf 'neutral release build: missing required command %s\n' \
      "${command_name}" >&2
    exit 1
  }
done

operator_home="$(getent passwd "$(id -u)" | cut -d: -f6)"
[[ -n "${operator_home}" ]] || {
  printf 'neutral release build: could not resolve operator home\n' >&2
  exit 1
}
rustup_bin="${RUSTUP_BIN:-$(command -v rustup || true)}"
if [[ -z "${rustup_bin}" && -x "${operator_home}/.cargo/bin/rustup" ]]; then
  rustup_bin="${operator_home}/.cargo/bin/rustup"
fi
[[ -n "${rustup_bin}" && -x "${rustup_bin}" ]] || {
  printf 'neutral release build: rustup is unavailable\n' >&2
  exit 1
}
cargo_home_host="${CARGO_HOME:-${operator_home}/.cargo}"
rustup_home_host="$("${rustup_bin}" show home)"
toolchain_cargo_host="$("${rustup_bin}" which cargo --toolchain 1.96.0)"
toolchain_rustc_host="$("${rustup_bin}" which rustc --toolchain 1.96.0)"

for required_dir in "${cargo_home_host}" "${rustup_home_host}"; do
  [[ -d "${required_dir}" ]] || {
    printf 'neutral release build: Rust cache/toolchain directory is unavailable\n' >&2
    exit 1
  }
done

case "${toolchain_cargo_host}" in
  "${rustup_home_host}"/*)
    toolchain_cargo_sandbox="/media/rustup/${toolchain_cargo_host#"${rustup_home_host}"/}"
    ;;
  *)
    printf 'neutral release build: cargo is outside the pinned rustup tree\n' >&2
    exit 1
    ;;
esac
case "${toolchain_rustc_host}" in
  "${rustup_home_host}"/*)
    toolchain_rustc_sandbox="/media/rustup/${toolchain_rustc_host#"${rustup_home_host}"/}"
    ;;
  *)
    printf 'neutral release build: rustc is outside the pinned rustup tree\n' >&2
    exit 1
    ;;
esac

rustc_version="$("${toolchain_rustc_host}" --version)"
case "${rustc_version}" in
  'rustc 1.96.0 '*) ;;
  *)
    printf 'neutral release build: pinned rustc 1.96.0 is required\n' >&2
    exit 1
    ;;
esac

# Fetching only populates Cargo's external cache. Compilation happens later in
# an offline, fixed-path sandbox, so host checkout/cache paths cannot become
# vendored OpenSSL prefixes in the release ELF.
"${toolchain_cargo_host}" fetch --locked --manifest-path "${rust_dir}/Cargo.toml"

temp_base="${TMPDIR:-/tmp}"
host_target="$(mktemp -d "${temp_base}/jbl-aura-neutral-target.XXXXXX")"
cleanup() {
  case "${host_target}" in
    "${temp_base}"/jbl-aura-neutral-target.*)
      rm -rf -- "${host_target}"
      ;;
  esac
}
trap cleanup EXIT

toolchain_bin_sandbox="$(dirname -- "${toolchain_cargo_sandbox}")"
neutral_rustflags='--remap-path-prefix=/mnt=/src --remap-path-prefix=/media/cargo=/cargo --remap-path-prefix=/media/rustup=/rustup'

bwrap \
  --die-with-parent \
  --new-session \
  --unshare-net \
  --clearenv \
  --ro-bind / / \
  --proc /proc \
  --dev-bind /dev /dev \
  --tmpfs /home \
  --tmpfs /tmp \
  --tmpfs /media \
  --dir /media/cargo \
  --dir /media/rustup \
  --bind "${cargo_home_host}" /media/cargo \
  --ro-bind "${rustup_home_host}" /media/rustup \
  --ro-bind "${repo_dir}" /mnt \
  --tmpfs /opt \
  --dir /opt/jbl-aura-build \
  --bind "${host_target}" /opt/jbl-aura-build \
  --setenv CARGO_HOME /media/cargo \
  --setenv RUSTUP_HOME /media/rustup \
  --setenv CARGO_TARGET_DIR /opt/jbl-aura-build \
  --setenv CARGO_NET_OFFLINE true \
  --setenv RUSTC "${toolchain_rustc_sandbox}" \
  --setenv RUSTFLAGS "${neutral_rustflags}" \
  --setenv SOURCE_DATE_EPOCH 1 \
  --setenv LC_ALL C \
  --setenv PATH "${toolchain_bin_sandbox}:/usr/bin:/bin" \
  --chdir /mnt \
  -- "${toolchain_cargo_sandbox}" build \
    --locked --offline --release --manifest-path /mnt/rust/Cargo.toml

artifact="${host_target}/release/jbl-aura-link"
"${repo_dir}/tests/release_privacy.sh" "${artifact}"

output_binary="$(realpath -m -- "${output_binary}")"
mkdir -p -- "$(dirname -- "${output_binary}")"
install -m 0755 -- "${artifact}" "${output_binary}"
printf 'neutral release build: PASS artifact=%s\n' "${output_binary}"
