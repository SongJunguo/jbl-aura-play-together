# Rust Ubuntu mainline (alpha)

This crate builds one Ubuntu executable from modular source. It is the v0.5
implementation mainline. The verified Python/BlueZ v0.4 path remains installed
as an explicitly selected fallback while the Rust release gate is still open;
the controller never switches to it automatically after a rejected or
uncertain Rust action.

The Rust program now contains the native Aura transport, whole-pair controller,
loopback Web service, local CLI client and Ubuntu user-service boundary. The
current sanitized hardware-verified scope includes:

- direct, proxy-free mTLS connection to a JBL Authentics 300;
- runtime client certificate/private key, never embedded in the executable;
- exact server-certificate SHA-256 pin checked before sending HTTP data;
- independent UPnP product check requiring `JBL Authentics 300`;
- sanitized `getDeviceInfo` projection;
- exact private member-identity validation of a ready two-member
  `getAuraCastGroupInfo` projection;
- bounded timeouts and a one-MiB response limit;
- accepted native Rust `start` and `stop` actions;
- an explicit recovery that first completed safe diagnosis, then mapped the
  paired stable Aura identity to the exact fresh FDDF random GATT identity and
  returned to managed `ready`;
- two no-button cold `start` rounds: managed status reported `br_edr` during the
  first and ended `le` during the second;
- normal `stop` through the retained native session in approximately 0.44 and
  0.57 seconds.

The Bluetooth object mapping needs a precise qualification. A direct connection
attempt to the discovered LE `Device1` failed. BlueZ could be nudged through the
paired-and-trusted stable public object, but the vendor GATT service then
appeared on the unique connected random object. Rust adopted that object only
after its fresh FDDF payload exactly matched the expected PID and embedded
stable identity. A `br_edr` lifecycle label is therefore an observed managed
transport report, not permission to skip the random-object identity gate.

The exact private-identity gate, dedicated no-pool HTTPS Play Together write
transport, native FDDF/GATT path, receiver-first STOP sequence, uncertain
outcome latch, `start`/`stop` service commands and explicit `recover-stop` are
wired end to end. They are covered by offline fixtures, and the lifecycle
claims above additionally passed controlled real-speaker actions.

The approximately 03:45 full-song attempt cannot be used as an acoustic gate:
Home Centre issued an automatic STOP in the same experimental window, so the
reported JBL-only audio was contaminated by a concurrent writer. A later clean
EOF-fixed trial used the Home-flow-only transaction without `7957`. Aura AA ON
and JBL Wi-Fi ENTER were accepted and managed state became `linked`; after a
`15`-second wait, JBL-only network playback still produced no Aura audio. The
captured official Home flow used Android A2DP into Aura, with Aura PRIMARY and
JBL RECEIVER. The no-`7957` design is therefore refuted for the project's
opposite JBL-source direction. The exact-GATT implementation then reintroduced
Assistant `7957` JBL-broadcaster semantics alongside Aura receiver semantics.
START was accepted and a JBL-only network source at requested `5%` was
explicitly confirmed on both speakers—the first Rust acoustic pass. Normal STOP
failed outcome-unknown on `aura_ack_timeout`; explicit recovery returned
accepted/`ready` within `13` seconds. After the fresh-bearer release fix, round
two restarted the service, released phone Bluetooth ownership, repeated the
accepted requested-`5%` dual-audio START, and completed one ordinary idle STOP
as accepted/`ready` in approximately `43` seconds without recovery. Sound
testing stopped after the two agreed successes. P0/release remains open because
`7951`, the unhit A2DP `wake_then_stable` subpath and remaining release gates
are unresolved. The
implementation is a cross-state-machine composition, not one official UI
sequence.

The later strict GENA trial opened the narrow callback firewall rule and armed
SUBSCRIBE before the exact GATT `7957` write, but this firmware delivered no
`7951` callback within 15 seconds. Production START ended
`jbl_broadcast_result_timed_out` and the pair was normalized through legacy
GATT; the installed rule is not protocol-success evidence. Runtime
configuration therefore has a closed
`JBL_BROADCAST_CONFIRMATION=ack|gena` choice and defaults this exact pair to
`ack`. In `ack` mode a successful action is reported as
`accepted_unconfirmed` with evidence `broadcast_acknowledgement_only`; only
`gena` action 33/34 may report `accepted` with
`broadcast_business_notification`. Managed `linked` records the last accepted
controller action and is never, by itself, a 7951 or acoustic claim.

NativePair START/STOP never use an idempotent shortcut. Managed linked/ready,
healthy matching lifecycle, verified membership and a resolved Aura route do
not prove current device roles, so every native START/STOP executes the backend.
Only the Legacy held session retains same-session idempotence. Repeated shutdown
after managed offline remains a local no-op.

The live counterexample returned native START as idempotent without writing and
left only JBL audible. Corrective STOP returned `accepted_unconfirmed` in
`46.71` seconds; the first
real START rejected-before-send clean in `49.76` seconds, and one bounded retry
returned `accepted_unconfirmed` in `48.56` seconds. The player was idle
immediately afterward, so
two-speaker silence was not initially a grouping result. Playback then resumed;
JBL reported `state=playing`, `volume=20`, while the user confirmed JBL-only
audio and a silent Aura. This ACK-only accepted START failed its acoustic check.
It does not erase the two historical successful Rust rounds; it shows that the
current compatibility transaction is unstable.

The deep-standby HCI trace shows Android BR/EDR A2DP auto reconnect, stored
link-key authentication/encryption and AVDTP Open before FDDF appears about
`2.5` seconds later; App LE reads occur later still. The wake module is now in
the production artifact. Its default cold chain is one stable raw attempt, one
eligible A2DP ConnectProfile attempt (`20` seconds), a `30`-second fresh FDDF
exact identity/PID gate, DisconnectProfile plus `5`-second release confirmation,
one stable raw retry, then the original LE fallback. One `150`-second outer
deadline bounds the whole chain; missing release confirmation fails before role
writes. Offline gates pass, and the no-button cold path has now completed once
through `fresh_le`: START `122.15` seconds accepted-unconfirmed/linked,
verified/healthy two-member status, then STOP
`15.89` seconds accepted-unconfirmed/ready with a clean journal and
`NRestarts=0`. No phone App participated, but no ADB phone-state evidence was
collected. The A2DP `wake_then_stable` subpath was not hit and remains unproven.

The current totals are `393` library tests plus `10` CLI tests, plus the FIFO
private-file helper gate `1/1`. Audit,
dependency-deny, fallback, privacy and neutral-build gates pass; compatibility
evidence mode is complete.

The same executable now also contains an independently gated JBL OneOS control
surface for the exact Authentics 300. It uses the fixed official-App bearer
selection evidence, strict SOAP/JSON parsers and the shared single-writer lock.
Hardware-verified direct controls are volume `0..9`, absolute mute, dynamic
`AUX/USB/BT` source selection and the four non-custom seven-band EQ presets.
Bluetooth-source Play/Pause remains evidence-required: with no active Bluetooth
media session, exact UPnP Play returned SOAP fault 501 and the App has no
fallback. Product settings remain unavailable because the tested firmware
returned no settings map. See
[`JBL_ONE_UBUNTU_PORT_PLAN.zh-CN.md`](../docs/JBL_ONE_UBUNTU_PORT_PLAN.zh-CN.md)
for the experiment ledger and safety boundaries.

The tested device uses X.509 v1 certificates that the inspected pure-Rust
verifier rejects. The Ubuntu candidate therefore uses vendored OpenSSL and an
in-handshake certificate verification callback. The callback accepts only the
exact configured peer DER SHA-256 pin; a mismatch terminates the handshake
before any HTTP request. This avoids the weaker post-handshake-only pin design.

## Command and service boundary

The installed command name is deliberately `jbl-aura-link-rust`, distinct from
the retained v0.4 `jbl-aura-link` launcher:

```text
jbl-aura-link-rust serve
jbl-aura-link-rust start
jbl-aura-link-rust stop
jbl-aura-link-rust status
jbl-aura-link-rust recover-stop --confirm
jbl-aura-link-rust doctor
jbl-aura-link-rust group
jbl-aura-link-rust discover --json
jbl-aura-link-rust media --json
jbl-aura-link-rust inspect --json
jbl-aura-link-rust capabilities --json
jbl-aura-link-rust volume-set 9 --confirm --json
jbl-aura-link-rust mute-set on --confirm --json
jbl-aura-link-rust mute-set off --confirm --json
jbl-aura-link-rust source-set aux --confirm --json
jbl-aura-link-rust source-set bluetooth --confirm --json
jbl-aura-link-rust eq-preset-set vocal --confirm --json
jbl-aura-link-rust eq-preset-set signature --confirm --json
```

`serve` is the only public executable path that constructs the native mutable
backend. Before construction it takes both shared owner-only v0.4/Rust locks
under `$XDG_RUNTIME_DIR/jbl-aura-link`: `operation.lock` excludes every v0.4
public entry and `session.lock` excludes the persistent v0.4 supervisor. It
also honors v0.4's owner-only systemd launch reservation, so it cannot cross
the deliberate lock hand-off window. It then owns one controller and one
loopback listener at `127.0.0.1:8096`. This is the Rust
daily-control port; Music Assistant remains on its separate `8095` port.
Service startup does not start Play Together, discover Aura, or send a role
command.

The same page now contains a JBL local-control card. `GET /api/jbl/status`
combines sanitized media, inspection, capabilities, dynamic source targets and
the recognized active EQ. Four exact-confirm POST routes expose only the
hardware-verified volume, mute, source and non-custom EQ operations. They share
the service actor's one `DirectControlLock` and revision with Play Together;
the daemon never reacquires or bypasses its own lock. Play/Pause, product
settings and raw command/payload routes do not exist. Unknown or unavailable
state disables the corresponding controls.

The Web snapshot has one fixed ten-request plan: one pinned identity read, one
model check, one UPnP playback read and seven OneOS inspection reads. Its
dedicated client clamps each request to 500 ms, so an unavailable or malformed
device cannot hold the single actor for the former many-minute worst case. A
successful snapshot is reused for at most two seconds. If the first read-only
plan reports only device unavailability or rejection, the service waits 100 ms
and repeats that complete plan once; it never retries a mutation. Thus a normal
cold refresh is bounded at about five seconds and that single recovery path at
about 10.1 seconds. This display snapshot is a best-effort, non-atomic view and
may be up to two seconds old. Every mutation independently re-reads its exact
identity, model, current state and safety preconditions instead of trusting the
display cache.

Every Web direct action durably marks the shared uncertainty journal before
device I/O. A weak or unknown result keeps that marker, disables subsequent JBL
snapshot/control and also blocks ordinary Pair mutations. Successful explicit
`recover-stop` clears the shared marker; known direct outcomes clear it
normally. Restarting the service cannot silently forget an uncertain direct
write.

While `jbl-aura-link-rust.service` is active it owns the shared operation and
session locks for its whole lifetime. Direct mutation CLI commands therefore
fail with `AlreadyRunning` by design; use the authenticated Web page for daily
volume/mute/source/EQ changes. Read-only CLI commands remain available. Stop
the service only as an explicit maintenance action, since graceful service
shutdown may normalize Play Together state before releasing its transports.

`start`, `stop`, `status` and `recover-stop` never load device configuration or
construct a backend. They use a direct TCP connection fixed to IPv4 loopback,
so proxy environment variables and redirects cannot affect them. A missing
listener permits exactly one bounded
`systemctl --user start jbl-aura-link-rust.service`; malformed or hung local
responses never launch a second controller. A mutation fetches a fresh CSRF
cookie and controller revision, sends one POST with `If-Match`, enforces one
absolute I/O deadline and a response cap, and does not retry HTTP 409 or an
uncertain action.

Recovery is intentionally not a Web-page button. The CLI requires the exact
`recover-stop --confirm` spelling and calls a hidden loopback route with an
additional exact confirmation body. The controller permits it only after an
uncertain action or at least two consecutive pre-send failures, and performs at
most one teardown, verified rescan and receiver-first STOP normalization.

Before every mutable backend call, the service durably writes a fixed-format,
non-identifying pending record under
`~/.local/state/jbl-aura-link-rust/uncertainty.state`. It clears that record only
after an accepted lifecycle result and fresh retained-membership postcondition.
A crash at a device-write boundary therefore restarts in unresolved state and
blocks ordinary `start`, `stop` and shutdown writes until the explicit recovery
gate succeeds. A corrupt, symlinked, broadly readable or unwritable journal
fails closed before backend mutation.

The crash boundary was exercised, not merely inferred from a unit test. An
older timeout construction caused a real process panic after the pending record
had been committed; reopening the service preserved the unresolved action and
blocked ordinary writes. The timeout implementation was fixed, and explicit
recovery remained necessary. Separately, a real FDDF advertising-window miss
rejected a `stop` before the first device write and returned the journal to
clean. A bounded classic-connect nudge was followed by fresh FDDF visibility;
the subsequent identity-checked recovery was accepted and ended `ready`.

`doctor` and `group` are separate read-only direct checks. Their output and all
local-service JSON use closed, sanitized vocabularies; no address, token,
credential path or raw backend error is representable.

`discover`, `media`, `inspect` and `capabilities` are also read-only. Discovery
uses the existing Avahi D-Bus service for one fixed five-second
`_jbl-product._tcp` window and returns only candidate cardinality, address-family
presence and fixed TXT-field presence; it does not expose or select a device.

Every direct mutation requires `--confirm` and acquires the same v0.4/Rust
operation and session locks before loading configuration or touching the
device. Only read-only preconditions may retry, at most three times and only
for `NetworkUnreachable`; a write, readback or bearer switch is never retried.
Volume is hard-capped at `9`. Source changes wait once for the measured 350 ms
publication delay and then read once; the speaker clears mute while changing
source, so the volume cap—not mute—is the safety invariant. EQ preset writes
reuse the device-returned seven-band ID, `fs` and `gain` arrays and cannot
construct `CUSTOMIZE` or arbitrary filter data.

Play/Pause remains an internal evidence fixture, not a production CLI command.
The exact official UPnP path returned SOAP fault 501 without an active
Bluetooth media session, and the official App has no alternate bearer fallback.

OneOS command traffic is authenticated with pinned mTLS. The device's port
59152 UPnP RenderingControl/AVTransport service is different: it is plain,
unauthenticated HTTP. Pinned-mTLS identity checks before and after a UPnP
transaction, the fixed IP literal and exact model check reduce accidental
misdelivery, but they do not cryptographically bind, encrypt or authenticate
the intervening UPnP write/readback. Treat the local network as part of the
trust boundary; an on-path LAN attacker could still alter that traffic.

SIGINT and SIGTERM handlers perform only one atomic store. After the listener
returns, the service makes one bounded controller shutdown attempt. If a prior
write is unresolved, the controller blocks another role write and process
teardown only releases local resources.

## Build and test

The repository pins Rust 1.96.0. Ensure the rustup shims are first on `PATH`
and confirm `rustc --version` before building. From the repository root:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --locked --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path rust/Cargo.toml
cargo build --locked --release --manifest-path rust/Cargo.toml
```

Vendored OpenSSL avoids a system `libssl-dev` build dependency and keeps the
release easier to move between compatible Ubuntu installations.

The ordinary Cargo command above is suitable for local development, but not
for a published artifact. OpenSSL's vendored C build records its absolute
installation prefix independently of Rust's `--remap-path-prefix`; building in
a random checkout or temporary target therefore leaks that build location into
the ELF. Ubuntu release artifacts must instead use Bubblewrap's fixed neutral
filesystem view:

```bash
./rust/build-neutral-release.sh
```

The script fetches the locked crates, then compiles offline with an empty
environment, read-only source/toolchain mounts, no network namespace, and the
fixed path-neutral `/opt/jbl-aura-build` OpenSSL prefix. The host-owned temporary
directory mounted there is private to the invoking user; it is not claimed to
be root-owned. Bubblewrap is used here for deterministic path/environment
neutralization, not as a security boundary against malicious build scripts or
as a claim of bit-for-bit reproducibility. The script runs the artifact
privacy/ABI gate before copying the executable to
`rust/target/neutral/jbl-aura-link`. A different output filename may be passed
as the only argument. `bubblewrap`, `binutils`, `file`, and `ripgrep` are build
gate dependencies, not runtime dependencies.

Cargo and the neutral build intentionally retain the package artifact name
`jbl-aura-link`. The Rust installer copies that reviewed ELF to the independent
installed name `jbl-aura-link-rust`; it never overwrites the v0.4 launcher.

## Private runtime configuration

For direct `doctor`, `group`, or manual `serve`, the automatic configuration
path remains:

```text
${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link-rust/devices.env
```

Override it with `JBL_AURA_CONFIG` or `--config PATH`. Either override is
required when named and fails if unavailable; only an absent automatic default
may fall back to complete environment configuration. The parser uses a fixed
key allowlist and never evaluates shell syntax. The configuration, certificate,
and private key must be owner-only regular files; Linux opens them with
`openat2(RESOLVE_NO_SYMLINKS)` so final and parent symlinks, special files,
other owners, broad POSIX modes and oversized files are rejected. Relative
credential paths are resolved from the configuration directory.

Values shown below are placeholders; never put real material in this repository
or a shell history:

```text
JBL_IP=<device-ip-literal>
JBL_LOCAL_API_CERT=<private-client-certificate-path>
JBL_LOCAL_API_KEY=<private-client-key-path>
JBL_LOCAL_API_TLS_SHA256=<private-device-certificate-sha256>
JBL_EXPECTED_MODEL=JBL Authentics 300
JBL_BT_MAC=<private-jbl-bluetooth-address>
AURA_BT_MAC=<private-aura-bluetooth-address>
```

`JBL_BT_MAC` and `AURA_BT_MAC` are mandatory private identity anchors. Each
accepts colon-separated, hyphen-separated, or plain 12-hexadecimal input. The
two addresses must be different, valid and plausible unicast Bluetooth
addresses. They are normalized only for internal comparison and are never
printed, serialized or logged.

The same keys can be supplied as process-environment overrides for controlled
testing. Output contains only
canonical model/role, safe version and allowlisted channel fields; custom/unknown
member names, group IDs, member IDs, addresses and raw JSON are discarded. Raw
identity values are never serialized or logged. The pair configuration is
reported as ready only when `disabled=false`, exactly two member IDs match the
configured JBL and Aura addresses, and each corresponding member has its fixed
canonical `device_name`. Display names are not configurable identity evidence.

The final loopback status page is readable and exposes only the two sanitized
members, allowlisted channel data, `last_action` and `age_ms`; CSP, Host/Origin
and CSRF protections remain enabled.

`getAuraCastGroupInfo` describes persistent membership configuration, not the
current audible link. The same ready two-member response can remain after a
successful stop, so `expected_pair_configured=true` and
`pair_configuration=ready` must never be presented as proof that both speakers
are currently linked or producing sound. No empty stopped schema is assumed.

This TLS/file backend is currently Ubuntu-only. Windows credential storage and
TLS behavior are deliberately deferred until after Ubuntu acceptance.

## Install the independent Rust user service

Build the reviewed neutral artifact first, then install it without starting it:

```bash
./rust/build-neutral-release.sh
./scripts/install-rust-user-service.sh
```

The installer creates these Rust-only entries and leaves every v0.4 file and
unit untouched:

```text
~/.local/bin/jbl-aura-link-rust
~/.config/jbl-aura-link-rust/devices.env
~/.config/systemd/user/jbl-aura-link-rust.service
~/.local/state/jbl-aura-link-rust/
```

The new private config is initialized from placeholder-only
`config/rust-devices.env.example` with mode `0600`; populate it and keep the
certificate/key beside it as owner-only files. The user unit passes this path
explicitly rather than assuming that the v0.4 config contains Rust mTLS fields.
The default install neither starts nor enables the unit, avoiding a restart
loop while placeholder config is still present. After the private values have
been reviewed, explicitly disable and shut down the v0.4 unit before enabling
Rust. The installer refuses to enable Rust while the v0.4 unit is enabled:

```bash
jbl-aura-link shutdown
systemctl --user disable --now jbl-aura-link-session.service
./scripts/install-rust-user-service.sh --enable
systemctl --user start jbl-aura-link-rust.service
jbl-aura-link-rust status
```

To return to v0.4, issue a bounded Rust `stop`, stop and disable the Rust unit,
then enable the v0.4 unit. Shared locks also fail closed if either executable is
started manually during the hand-off; they do not perform automatic failover.

The unit uses `Restart=on-failure`, a loopback-only TCP listener and an
address-family allowlist containing only UNIX, IPv4/IPv6 and the required
Bluetooth family. It retains compatible read-only home/system protection and
`TimeoutStopSec=600s`, matching the local mutation deadline and covering the
three bounded stable-bearer attempts, the 150-second strict LE fallback, one
bounded STOP and membership checks. It does not reference or replace
`jbl-aura-link-session.service`.

Controlled hardware acceptance has covered native Rust
`start -> status -> stop`, both tested cold-discovery bearers, crash-persistent
uncertainty and explicit recovery. The 03:45 acoustic attempt was invalidated
by an automatic Home Centre STOP. The later clean default transaction omitted
`7957`, reached accepted Aura AA ON plus JBL Wi-Fi ENTER and local `linked`, but
after the EOF fix and a `15`-second wait still produced JBL-only audio. Fixed
`10.5`/`15`-second delays are not repairs. Exact GATT `7957` then produced the
first target-direction START acoustic pass, while HTTPS `7957` returned HTTP
`200`/`unknown command`. Four GATT writes were ACKed without `7951`; normal STOP
failed on an Aura ACK timeout and explicit recovery returned ready. The
fresh-bearer release fix then produced a second START acoustic pass and one
ordinary approximately `43`-second STOP accepted/ready without recovery. Keep
the v0.4 Python/BlueZ fallback installed and select it only explicitly; there is
no automatic failover. Final test totals and neutral ELF
evidence are frozen: `258` library plus `8` CLI tests (`266` main), FIFO
private-file helper `1/1`, audit/deny/fallback/privacy/neutral gates,
`8,284,440` bytes,
`GLIBC_2.34`, and only `libc`/`libgcc`.
Artifact, installed-file and running-process digests match; the value remains
internal. After restart and one read-only status, the enabled/active user service
had `NRestarts=0` and managed unknown/offline. Its `15`-second restart-idle sample
measured `8,828 KiB` RSS, one thread, `15` fds and `0.0667%` average CPU (`1`
tick).
