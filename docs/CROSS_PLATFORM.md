# Windows 11 and Ubuntu architecture

Ubuntu 22.04 is the active implementation and acceptance platform. Windows 11
is the planned second platform after the Ubuntu version is stable and the
repository is moved there. Windows support is not a parallel v0.5 blocker and
is not yet claimed as verified.

## Portable core

The following logic must have no BlueZ, systemd, registry, or shell dependency:

- OneOS PL and Aura AA frame encoding/decoding;
- sanitized device, playback, and two-member group models;
- evidence-level and state reduction;
- mTLS HTTPS, UPnP/SOAP, response limits, TLS pinning, and retry policy;
- configuration validation with runtime-only credential paths.

## Platform transports

| Capability | Ubuntu | Windows 11 |
|---|---|---|
| JBL LAN API | Rust/OpenSSL/UPnP; Python reference fallback | Rust transport adapted and revalidated |
| mDNS | Planned Rust library with explicit device selection | Same model, Windows-tested backend |
| Aura live LE discovery | Native Rust/BlueZ FDDF resolver implemented; typed Python D-Bus resolver retained as v0.4 fallback | WinRT/BLE backend after Ubuntu |
| Persistent GATT control | Native Rust/BlueZ UUID discovery, notify and fixed AA writes implemented; gatttool session retained as v0.4 fallback | WinRT/BLE backend after Ubuntu |
| Service lifecycle | systemd user unit | Windows Service or Task Scheduler wrapper |
| Local CLI | One Rust executable | Separate Rust `.exe` after Ubuntu acceptance |
| Home Assistant | Python adapter on the HA host | Browser/API client; HA core still runs on its supported host |

Windows must not emulate the Ubuntu command line by invoking unavailable tools.
The shared state machine calls a transport interface; each backend reports the
same bounded error categories and sanitized postconditions.

## Deferred first Windows milestone

After Ubuntu acceptance and repository migration, the first Windows experiment
is deliberately read-only/control-only:

1. discover and authenticate the JBL over LAN;
2. read sanitized device and Play Together group state;
3. scan the Aura's live FDDF advertisement and verify PID plus embedded stable
   identity without persisting its rotating address;
4. connect through WinRT/Bleak, subscribe to the expected notification, and
   confirm transport capability without changing group state;
5. only after the above passes, perform one explicitly authorized ON/OFF test
   with device-reported and human postconditions.

The existing Ubuntu backend stays available throughout this work so a Windows
experiment cannot remove the already reproduced recovery path.

## Private runtime material

Authorized private experiments may use credentials and device identifiers from
the inspected reference projects. Store them outside the public checkout and
restrict access with POSIX permissions or a Windows ACL. Never place them in a
wheel, PyInstaller executable, Rust binary, container, log, fixture, CI secret,
release archive, or Git commit.

## Rust boundary

Rust is the current Ubuntu product mainline and the planned basis for a later
Windows executable, but it is not accepted merely because it compiles. The Rust client must use runtime
credentials, certificate/fingerprint pinning, direct/no-proxy LAN access,
bounded responses, sanitized logs, and the same fixtures and hardware evidence
as the Python reference. The native Rust Bluetooth backend has passed identity,
transport-ACK and recovery checkpoints. Its approximately 03:45 acoustic
attempt was contaminated by an automatic Home Centre STOP and carries no
protocol conclusion. A later clean default transaction omitted `7957`; Aura AA
ON and JBL Wi-Fi ENTER were accepted and local state became `linked`. After the
EOF fix, a `15`-second wait before JBL-only network playback still produced no
Aura audio. The official Home flow instead used Aura as A2DP source/PRIMARY and
JBL as RECEIVER. The Ubuntu candidate therefore reintroduces the separate
Assistant `7957` JBL-broadcaster semantics as a direction-aware composition
with Aura AA receiver semantics, not as one official UI sequence. Exact GATT
`0x002a` produced the first Rust START acoustic pass, but ordinary STOP failed
`aura_ack_timeout`/outcome-unknown and required explicit recovery. After the
fresh-bearer fix, round two repeated the dual-audio START and completed one
ordinary approximately `43`-second STOP accepted/ready without recovery. Sound
testing stopped after two successes; P0/release still awaits `7951`, deep-
standby subpath evidence and remaining gates. The production no-button cold path
now has one silent hardware pass via `fresh_le` inside `150` seconds; the A2DP
`wake_then_stable` subpath was not hit. The Python/BlueZ v0.4 path stays
available.
