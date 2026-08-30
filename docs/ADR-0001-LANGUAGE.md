# ADR-0001: Rust-first product with a verified Python fallback

- Status: amended for Ubuntu-first, Windows-second delivery
- Date: 2026-08-29

## Decision

Build the Ubuntu product, state machine, CLI and later Web UI in Rust 1.96.0.
Retain the existing Python/BlueZ implementation as the verified reference and
fallback until Rust reproduces its Ubuntu hardware behavior and failure cases.
Bash and later PowerShell remain thin installation/service glue only. Windows
parity is evaluated after the accepted Ubuntu implementation is moved there.

## Why

This project is dominated by bounded network requests, Bluetooth discovery and
control, local state, and service integration. It is not compute-bound. The
deployed Python supervisor uses little memory, so a language rewrite would not
improve the current bottleneck: device availability and vendor-state
reliability.

Home Assistant components are Python-based, but they will be a thin client of
the Rust local service rather than a second OneOS/Play Together state machine.

The inspected Rust CLI demonstrates that a compact native client is feasible,
but a rewrite would still need to solve the same firmware-specific response
semantics, runtime credential handling, TLS verification, live rotating-address
discovery, and Play Together recovery. Rust by itself does not make those
states correct.

The eventual cross-platform requirement is a real reason to evaluate Rust, but
not a reason to split current work across two operating systems. First prove the
Rust LAN and Ubuntu Bluetooth paths against the existing Ubuntu reference. The
Windows transport is implemented later, using the already frozen interfaces.

## Why Rust is now the product mainline

The user selected single-executable distribution and a later Windows port as
concrete requirements. The Rust LAN alpha has also passed real Authentics 300
model and two-member membership reads. Those reads satisfy the identity/config
evaluation gate, although later STOP evidence shows they are not a live-state
signal.
Rust is therefore the implementation mainline, while Python remains the
behavioral oracle and rollback rather than being deleted prematurely.

## Architecture consequence

New work moves protocol logic out of Bash into small Rust modules with stable
interfaces:

```text
CLI / future Home Assistant adapter
              |
       typed control interface
              |
  LAN OneOS + Ubuntu Aura transport
              |
       evidence/state reducer
```

No component may embed client credentials. Private experiments may use
authorized credentials from reference projects, but every implementation must
load them from an operator-owned runtime path and authenticate the device
through certificate or fingerprint pinning.

The Ubuntu development toolchain is pinned to Rust 1.96.0 by
`rust-toolchain.toml`, deliberately two stable releases behind the 1.98.0
current when selected. The project never follows the moving `stable` channel.
Cargo metadata uses the same Rust 1.96 minimum. Upgrade only for a relevant
security fix, a required dependency, or the later Windows port, and record the
reason with a locked regression run.

## Platform sequence

1. Freeze portable request/response models, error categories, evidence levels,
   and sanitized fixtures.
2. Keep the existing Python implementation frozen as the comparison fallback.
3. Harden the Rust read-only LAN/model/group client, then add writes only after
   differential parity.
4. Implement or wrap the Ubuntu Bluetooth backend. Keep the existing
   BlueZ/gatttool path as a compatibility fallback.
5. Require the Rust Ubuntu executable to survive cold discovery, FDDF gaps,
   competing-phone ownership, disconnects, restart recovery, and repeated
   start/stop tests before it replaces the reference path.
6. After Ubuntu acceptance, move the repository to Windows and implement the
   WinRT/BLE/service backend against the already frozen interfaces.
