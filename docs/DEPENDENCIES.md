# Dependency policy and inventory

Necessary dependencies are authorized for the Ubuntu implementation, tests,
packaging, and real-device validation. This is permission to install justified
packages, not permission to add unrelated development suites.

## Rust mainline

| Dependency | Purpose | Policy |
|---|---|---|
| rustup toolchain 1.96.0 | Reproducible compiler, Cargo, rustfmt and Clippy | Pinned by `rust-toolchain.toml`; do not follow moving stable |
| Cargo.lock dependencies | mTLS, HTTP, JSON, SHA-256 and bounded CLI logic | Commit the lock and build with `--locked` |
| vendored OpenSSL | Accept the tested device's X.509 v1 certificates, then enforce an exact DER SHA-256 pin | Built from the locked crate; avoids requiring Ubuntu `libssl-dev` |
| bluer 0.17.4 | Official BlueZ Rust interface for typed LE discovery, Device/GATT lifetime ownership, StartNotify and fixed-frame writes | Ubuntu-only native Aura transport; BSD-2-Clause; talks only to system `bluetoothd` |
| tokio 1.48.0 | Bounded BlueZ futures, active-scan windows and command/ACK deadlines | Current-thread runtime only; no worker pool |
| dbus 0.9.10 with vendored libdbus | Unifies bluer's D-Bus dependency without requiring host `libdbus-1-dev` | Statically compiled into the release artifact; raw D-Bus errors are never exposed |
| futures 0.3.31 | Typed BlueZ event and notification streams | Used only inside the native Aura transport |
| system C toolchain and Perl | Build vendored OpenSSL | Use existing Ubuntu tools; document any missing apt package before installing |
| Bubblewrap, binutils, file and ripgrep | Build under fixed neutral paths and inspect the release ELF without publishing host paths | Build/CI only; no runtime dependency |
| ShellCheck 0.8 or newer | Lint the thin Rust user-service installer and its offline render test | Build/CI only; no runtime dependency |
| cargo-audit 0.22.2 | RustSec vulnerability gate | Fixed tool version in local checks and CI; no silent advisory ignores |
| cargo-deny 0.20.2 | License, source, wildcard and advisory policy | Fixed tool version; policy is `rust/deny.toml` |
| cargo-about 0.9.2 | Generate the distributable dependency attribution and license-text notice | Fixed tool version; `rust/about.toml` is independent of cargo-deny policy |
| cargo-sbom 0.10.0 | Generate SPDX 2.3 and CycloneDX 1.6 software bills of materials | Fixed tool version; generation is locked and offline |

The frozen neutral executable is `8,284,440` bytes, requires at most
`GLIBC_2.34`, and dynamically links only the normal Ubuntu runtime loader,
`libc` and `libgcc`; it does not require a separate Python or OpenSSL runtime.
The installed digest matches the reviewed artifact, while the digest value is
kept in release-internal evidence rather than public prose.

## Public notice and SBOM regeneration

Install the two release-metadata tools with the repository-pinned Rust
toolchain, then run the generator from any directory:

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli
cargo install cargo-sbom --version 0.10.0 --locked
./scripts/generate-rust-release-metadata.sh
```

The generator first proves that locked, offline Cargo metadata resolves. It
then rewrites `THIRD_PARTY_LICENSES.md`, the SPDX 2.3 JSON document, and the
CycloneDX 1.6 JSON document under `sbom/`. It verifies both JSON schemas' core
identity fields, checks that `Cargo.lock` did not change, rejects private paths,
LAN/device addresses and credential-shaped output, and finally runs the public
tree privacy gate. Include all three generated artifacts with a release.

`cargo-deny` remains a dependency-policy gate only. A successful
`cargo deny check` neither generates nor replaces the third-party notice.

## Existing Ubuntu reference/fallback

| Dependency | Purpose |
|---|---|
| BlueZ and bluez-tools | Bluetooth discovery/control and current gatttool fallback |
| Python 3 | Verified persistent-session reference implementation |
| dbus-fast | Typed BlueZ D-Bus FDDF discovery |
| jq and xxd | Current Bash wrapper framing and state handling |
| PulseAudio utilities, when PulseAudio is used | Bounded Bluetooth-module arbitration and restoration |
| systemd user manager | Boot/session lifecycle |

On the development host, Python commands use the isolated `jbl-aura-re`
environment after inspecting `conda env list`. Conda base is not modified.

## Native Ubuntu Bluetooth build

The Rust Aura layer builds against pinned `bluer` and a vendored `libdbus`, so
it requires neither `libdbus-1-dev` nor Android tooling. BlueZ/bluetoothd remain
the normal Ubuntu runtime boundary. Do not install Android Studio, an emulator,
or unrelated SDKs for this path.

If apt elevation is required and no non-interactive sudo session exists, stop
and give the operator one explicit reviewed install command. Never handle the
operator's password.

## Strict GENA and firewall boundary

Default `JBL_BROADCAST_CONFIRMATION=ack` requires no inbound callback firewall
rule. Strict `gena` mode may require one narrowly scoped host rule for the
configured callback listener; changing the firewall always requires explicit
operator authorization. UFW is a host policy facility, not a project runtime
dependency.

The authorized narrow-rule production trial still ended
`jbl_broadcast_result_timed_out` and produced no `7951`. Installing a firewall
rule is therefore neither a protocol dependency nor success evidence. Do not
recommend a broad inbound rule, and do not make strict GENA the default for this
exact firmware.

The deep-standby wake module reuses the existing BlueZ/BR/EDR facilities and
adds no package dependency. It is production-integrated and present in the
neutral artifact. The overall no-button cold path has one hardware pass via
`fresh_le`; the A2DP `wake_then_stable` subpath was not hit and remains the only
wake-specific hardware gap. This does not add a dependency.

## Updating dependencies

1. state the requirement being solved;
2. prefer an existing dependency or standard library where reasonable;
3. inspect license, maintenance, MSRV, binary-size and security impact;
4. pin/update the lock file;
5. run format, Clippy, unit tests, release build and privacy checks;
6. record a toolchain or major dependency change in the changelog/ADR.
