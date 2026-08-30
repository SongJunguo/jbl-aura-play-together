#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(CDPATH='' cd -- "${script_dir}/.." && pwd -P)
binary_source="${repo_root}/rust/target/neutral/jbl-aura-link"
unit_source="${repo_root}/systemd/jbl-aura-link-rust.service.in"
config_example="${repo_root}/config/rust-devices.env.example"
enable_service=false

usage() {
    echo 'Usage: install-rust-user-service.sh [--binary PATH] [--enable]'
}

while (($# > 0)); do
    case "$1" in
        --binary)
            (($# >= 2)) || { usage >&2; exit 2; }
            binary_source=$2
            shift 2
            ;;
        --enable)
            [[ ${enable_service} == false ]] || { usage >&2; exit 2; }
            enable_service=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

[[ -n ${HOME:-} && ${HOME} = /* ]] || {
    echo 'install-rust-user-service: HOME must be an absolute path' >&2
    exit 1
}
[[ -f ${binary_source} && ! -L ${binary_source} && -x ${binary_source} ]] || {
    echo 'install-rust-user-service: the reviewed Rust executable is unavailable' >&2
    exit 1
}
[[ -f ${unit_source} && ! -L ${unit_source} ]] || {
    echo 'install-rust-user-service: the Rust user unit template is unavailable' >&2
    exit 1
}
[[ -f ${config_example} && ! -L ${config_example} ]] || {
    echo 'install-rust-user-service: the Rust private config template is unavailable' >&2
    exit 1
}
command -v install >/dev/null 2>&1 || {
    echo 'install-rust-user-service: install is unavailable' >&2
    exit 1
}
command -v systemctl >/dev/null 2>&1 || {
    echo 'install-rust-user-service: systemctl is unavailable' >&2
    exit 1
}
if [[ ${enable_service} == true ]] && \
    systemctl --user is-enabled --quiet jbl-aura-link-session.service >/dev/null 2>&1; then
    echo 'install-rust-user-service: disable jbl-aura-link-session.service before enabling Rust' >&2
    echo 'install-rust-user-service: both versions share exclusive device locks' >&2
    exit 1
fi

launcher_dir="${HOME}/.local/bin"
user_unit_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
rust_config_dir="${HOME}/.config/jbl-aura-link-rust"
rust_config="${rust_config_dir}/devices.env"
rust_state_dir="${HOME}/.local/state/jbl-aura-link-rust"
rust_launcher="${launcher_dir}/jbl-aura-link-rust"
rust_unit="${user_unit_dir}/jbl-aura-link-rust.service"

install -d -m 0755 -- "${launcher_dir}" "${user_unit_dir}"
install -d -m 0700 -- "${rust_config_dir}"
install -d -m 0700 -- "${rust_state_dir}"
install -m 0755 -- "${binary_source}" "${rust_launcher}"
install -m 0644 -- "${unit_source}" "${rust_unit}"
if [[ ! -e ${rust_config} && ! -L ${rust_config} ]]; then
    install -m 0600 -- "${config_example}" "${rust_config}"
fi

systemctl --user daemon-reload
if [[ ${enable_service} == true ]]; then
    # Deliberately omit --now. Enabling never changes grouping or opens the
    # backend. The shared locks protect direct/manual starts as well.
    systemctl --user enable jbl-aura-link-rust.service >/dev/null
    echo 'Rust Play Together user service installed and enabled; it was not started.'
else
    echo 'Rust Play Together user service installed but not enabled; review its private config first.'
fi
