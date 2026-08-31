# JBL Aura Play Together for Linux

[简体中文](README.zh-CN.md)

Unofficial, experimental Linux tooling that sends the vendor control sequence
used to associate a **JBL Authentics 300** with a **Harman Kardon Aura Studio 5**.

The project does one job: link or unlink those two speakers. It does **not**
include a music server, streaming provider, account login, cloud API, phone app,
or audio files.

## Verified result and honest boundary

On 2026-08-28, the sequence in this repository was reproduced on real hardware.
The Aura's Linux A2DP path was disconnected, the JBL ATT server acknowledged a
write carrying OneOS command `7957`, the Aura accepted vendor AA Play Together
ON, and a listener confirmed that both speakers were audible while one test
source was playing. No JBL `7951` application-level success notification was
captured.

The current exact firmware also produced no `7951` callback in a strict
SUBSCRIBE-before-GATT trial with the narrow callback firewall rule installed.
The Rust controller therefore exposes only `JBL_BROADCAST_CONFIRMATION=ack|gena`
and defaults this tested pair to `ack`. ACK mode reports
`accepted_unconfirmed` plus `broadcast_acknowledgement_only` with CLI exit `0`;
only a matching
GENA action 33/34 reports `accepted` plus
`broadcast_business_notification`. A managed `linked` state describes the last
accepted controller action, not a 7951 or acoustic proof.
It is also not permission for a NativePair idempotent shortcut: native START
and STOP always execute the backend, regardless of managed state, health,
lifecycle, verified membership or resolved Aura route. Only the Legacy held
session retains same-session idempotence; repeated offline shutdown is a local
no-op.

The live counterexample returned native START as idempotent without a write
while only JBL was audible. Corrective STOP returned `accepted_unconfirmed` in
`46.71` seconds; the
first real START rejected-before-send clean in `49.76` seconds, and one bounded
retry returned `accepted_unconfirmed` in `48.56` seconds. The test player was idle immediately after
the retry, so both speakers being silent is not a grouping failure. Playback
was then resumed directly; JBL reported `state=playing`, `volume=20`, but the
user confirmed that only JBL was audible and Aura remained silent. This round's
ACK-only accepted START did not form an audible link. It does not overturn the
two historical successful Rust rounds; it demonstrates that the current
compatibility transaction is not stable.

The source was then changed without claiming a new control transaction: the JBL
network source was stopped and the existing Aura A2DP bridge was started. The
test player reported JBL idle at volume `20` and Aura playing at volume
`20`. An initial impression that both speakers were audible did not persist;
after sustained listening for more than ten seconds, the user corrected the
result to Aura-only audio with JBL silent. The transient second sound is treated
as possible residual buffering, not an acoustic pass. The Aura queue and bridge
were stopped; both players ended idle and the bridge exited. Thus neither source
direction produced sustained two-speaker audio in this regression, and no new
practical solution is claimed.

The narrow callback firewall rule was explicitly authorized and installed, but
strict production START still ended `jbl_broadcast_result_timed_out` and was
followed by legacy-GATT normalization. The rule is not protocol-success
evidence. The latest HCI trace shows the earlier deep-standby wake began with
Android system BR/EDR A2DP auto reconnect, stored link-key authentication/
encryption and AVDTP Open; FDDF appeared about `2.5` seconds later and App LE
reads came later still. The wake module is now integrated in production and is
included in the latest neutral artifact. The default cold path performs one
stable raw-control attempt, then on an eligible failure performs one bounded
A2DP profile connect (`20` seconds), requires fresh exact FDDF identity/PID
within `30` seconds, disconnects the profile and confirms release within `5`
seconds, retries stable raw once, then retains the original LE fallback. All of
this shares one `150`-second outer deadline. If profile release is not confirmed,
startup fails before any role write. The latest silent no-button cold run began
with no active audio stream, a clean journal and no resolved ready-made BlueZ
device session. START completed in `122.15` seconds as
`accepted_unconfirmed`/`linked`; status reported the exact two-member pair
verified/healthy with Aura route `fresh_le`. STOP completed in `15.89` seconds
as `accepted_unconfirmed`/`ready`; the final journal was clean and
`NRestarts=0`. No phone App participated in the transaction, but there was no
ADB observation of phone state. This verifies the overall no-button cold path
within `150` seconds; the A2DP `wake_then_stable` subpath was not separately hit
or proven.

The current offline gate totals are `258` library tests plus `8` CLI tests
(`266` in the main harness), plus the FIFO private-file helper gate `1/1`;
audit, dependency-deny, fallback, privacy and neutral-build gates pass, and
compatibility evidence mode is complete.

The final neutral rebuild is `8,284,440` bytes, requires at most `GLIBC_2.34`,
and dynamically links only `libc` and `libgcc`. Installation/digest verification
for this rebuilt binary now passes: artifact, installed file and running process
digests match. After restart and one read-only status, the enabled/active service
had `NRestarts=0` and managed state unknown/offline. Its `15`-second restart-idle
sample measured `8,828 KiB` RSS, one thread, `15` file descriptors and `0.0667%`
average CPU (`1` tick). The readable loopback Web/status view
exposes only two sanitized members, allowlisted channel data, `last_action` and
`age_ms`; CSP and CSRF protections remain enabled.

Static interoperability analysis subsequently supplied controller-side
semantics for both writes. JBL One `2.7.9` builds `7957 action=1` with the
controlled JBL's own address as the broadcast address and waits for `7951`.
Harman Kardon One `2.6.11` constructs the exact Aura frame
`aa1304003c0101`, names token `0x3c` as AuraCast status, and classifies
`aa00021300` as a successful Set Device Info acknowledgement. These facts
strengthen the control-plane interpretation; they do not identify the on-air
LE Audio data plane.

On 2026-08-29, a separate live controller check opened JBL One's PartyTogether
page, discovered the Aura, placed it in the first receiver slot, and retained
that state after leaving and reopening the page. No music was started and no
fresh `7951` object was available in release logcat, so this is additional
controller-state evidence rather than a new acoustic or data-plane proof.

A later simultaneous UI, HCI and network capture resolved the complete official
home flow. Aura AA ON succeeded; one JBL `7937` GATT attempt was rejected as
Write Not Permitted; the successful JBL stage correlated with an encrypted
`95`-byte Wi-Fi TLS record and the App's OneWiFiSession `enterAuracast` call.
Removal used successful Aura AA OFF plus an encrypted `94`-byte record aligned
with `exitAuracast`. No `7957` step belonged to this home flow. Its audio source
was Android A2DP into Aura; Aura then reported PRIMARY and JBL RECEIVER, and
both were audible. TLS plaintext was not decrypted; command identity comes from
time, record length/direction and the exact static call graph. See the
[sanitized runtime evidence](docs/OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md).

The historical v0.4 compatibility sequence still uses `7957` from the separate
broadcaster-assistant path. It remains valid historical evidence, but it is not
presented as a byte-for-byte replay of the official PartyTogether home flow.

The same day exposed and resolved an important Linux lifecycle problem. A
one-shot `start` succeeded, but an immediate one-shot `stop` could not create
fresh control connections after the speakers changed role. Keeping both
control sessions open fixed that failure: three START operations and two STOP
operations then completed consecutively without another button press, App,
re-pair, or reconnect. Every Aura ON/OFF returned the App-defined
`aa00021300` success reply; JBL ENTER/EXIT returned `error_code=0`.

Version 0.4 adds automatic cold discovery without treating a rotating LE
address as identity. It consumes live BlueZ D-Bus events and requires Harman
FDDF Service Data, the expected PID, and the embedded stable address to match.
With the phone disconnected and no speaker button press, two real-hardware
`shutdown -> LE cold start -> stop` rounds completed. During one linked round,
a source was sent only to the JBL at 15% volume and the listener again confirmed
that both speakers were audible.

The negative result is retained: between those two passes, one 30-second scan
received 49 advertisement events but no Aura FDDF and therefore failed before
any role write. FDDF later returned within 10.7 seconds and the next cold start
passed. Cold start is thus proven possible without a button, but every immediate
scan is not claimed to succeed. Keeping the supervisor in `ready` after `stop`
remains the most reliable daily path.

Also on 2026-08-28, a second controlled run established an important host-side
condition: while a phone held the Aura connection, one Ubuntu attempt timed out.
After the phone disconnected, the combined procedure of temporarily suspending
PulseAudio's two Bluetooth modules, releasing the stale BlueZ session, and
waiting briefly completed the tested ON/OFF transactions. The CLI now performs
that guarded procedure automatically on PulseAudio hosts.

This is strong evidence for a working **vendor Play Together path**. It is not
proof of a standards-compliant BASS subscription:

- the Aura Broadcast Receive State probe was inconclusive (BASS requires an
  encrypted read, which the original probe did not prove it had established);
- a post-link DFFD sample cached by BlueZ decoded as `RECEIVER(2)` under an
  app-derived private enum, but its on-air freshness was not established and
  that enum is distinct from the OneOS broadcast-status namespace;
- no LE Audio ISO capture was made.

The CLI therefore reports transport acknowledgements and conservative
diagnostics. It never turns a successful GATT write into a claim that BASS is
active. See [Evidence](docs/EVIDENCE.md) and [Protocol](docs/PROTOCOL.md).

Development follows an explicit clean-room and language policy. See
[Repository working rules](AGENTS.md), [upstream feature intake](docs/UPSTREAM_INTAKE.md),
and [ADR-0001](docs/ADR-0001-LANGUAGE.md). Rust 1.96.0 is the current v0.5
product mainline; the verified Python/BlueZ implementation remains the
behavioral oracle and rollback until Rust passes the same hardware gates.

Development is Ubuntu-first. v0.5 must be completed and accepted on Ubuntu
22.04 before the repository is moved to Windows 11 for the second platform port.
Windows support is planned, not currently verified. See the normative
[project goal](docs/PROJECT_GOAL.md) and [platform architecture](docs/CROSS_PLATFORM.md).
The normative product requirements are maintained separately in
[the Chinese requirements specification](docs/REQUIREMENTS.zh-CN.md); contributor
rules do not substitute for that document.

The modular Rust v0.5 mainline builds as one Ubuntu executable and has now
passed a controlled native lifecycle checkpoint. Its read-only path matched
both private member identities exactly and reported the expected pair
configuration ready; a STOP comparison still proves that this retained
membership is not a live linked-state signal. Native Rust `start` and `stop`
were accepted, and two no-button cold starts respectively reported `br_edr`
during the first round and ended `le` during the second. The paired-and-trusted
stable public object only triggered BlueZ connection; Rust adopted the unique
connected random GATT object after exact FDDF PID/stable-identity matching.
Explicit recovery returned to `ready`, and two normal retained-session stops
completed in approximately 0.44 and 0.57 seconds.

The earlier full-song Rust attempt at approximately 03:45 is not a protocol
result: Home Centre issued an automatic STOP in the same experimental window,
so its JBL-only audio was contaminated by a concurrent writer. A later clean
trial used the Home-flow-only build without `7957`. After the EOF fix, Aura AA
ON and JBL Wi-Fi ENTER were accepted and local state became `linked`; the test
waited `15` seconds, then sent Music Assistant audio only to JBL. The user again
confirmed JBL audio and a silent Aura. This refutes the no-`7957` design for the
project's JBL-source direction and rules out the original `2`-second delay as
the sole cause; fixed `10.5`/`15`-second waits are not demonstrated repairs.
The next exact-GATT candidate reintroduced the separate Assistant `7957`
broadcaster semantics for JBL alongside Aura AA receiver semantics. After phone
control was released, START was accepted; Music Assistant targeted only JBL at
the requested `5%`, and the user confirmed both speakers. This is the first
Rust target-direction acoustic pass. HTTPS `7957` returned HTTP `200` with an
`unknown command` device result, while GATT handle `0x002a` was ACK-only and no
`7951` arrived. Normal STOP then failed outcome-unknown on an Aura ACK timeout;
explicit recovery returned accepted/`ready` within `13` seconds. After the
fresh-bearer release fix, round two restarted the service, released phone
Bluetooth ownership, repeated the JBL-only requested-`5%` dual-audio pass, and
then completed an ordinary idle STOP in approximately `43` seconds as
accepted/`ready` without recovery. Acoustic testing stopped after the two
agreed successes. P0/release is still incomplete: `7951` is unconfirmed and a
prior deep-standby case needed the phone's automatic connection to wake the
speaker. The new no-button cold run verifies the overall `fresh_le` fallback,
but not the specific A2DP `wake_then_stable` branch. This remains a directional
composition across two official state machines, not one official UI sequence.
The older v0.4 acoustic result remains separate evidence.
v0.4 also remains installed as an explicitly selected fallback; Rust
never switches to it automatically after a rejected or uncertain action. Both
versions honor shared owner-only operation/session locks, so they cannot own
the speakers concurrently. The Rust daily UI is on loopback port `8096`,
separate from Music Assistant on `8095`. See the
[Rust implementation notes](rust/README.md) and
[sanitized checkpoint evidence](docs/RUST_LAN_EVIDENCE_2026-08-30.md), plus the
[official-App/Rust contrast](docs/OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md).

The 8096 page now combines the existing Play Together card with sanitized JBL
media/inspection status and the four hardware-verified controls: volume 0–9,
absolute mute, dynamic AUX/USB/Bluetooth source and four non-custom EQ presets.
All writes share one actor lock, CSRF check and strong revision. Play/Pause,
product settings and arbitrary commands are intentionally absent.

The longer-term product goal has expanded beyond association-only tooling: a
local, open-source JBL One replacement that covers the useful capabilities of
the two closest public projects while retaining this repository's unique Play
Together backend. The proposed product requirements and honest feature matrix
are recorded in [Open JBL One requirements](docs/OPEN_JBL_ONE_REQUIREMENTS.zh-CN.md)
and [feature parity](docs/FEATURE_PARITY.zh-CN.md). A separate general-purpose
main repository is recommended so this v0.4 evidence history remains intact.

The active implementation keeps Play Together as the unique P0 capability, but
the evidence-closed Ubuntu JBL controls are now also present behind independent
gates. Hardware-verified functions are sanitized status, volume `0..9`,
absolute mute, dynamic `AUX/USB/BT` source selection and four
non-custom seven-band EQ presets. Bluetooth Play/Pause remains
evidence-required, and product-setting writes remain unavailable. The two
current execution plans are the
[Play Together Rust plan](docs/PLAY_TOGETHER_RUST_PLAN.zh-CN.md) and the
[JBL One Ubuntu port plan](docs/JBL_ONE_UBUNTU_PORT_PLAN.zh-CN.md); neither may
weaken the other's identity, privacy or outcome-unknown rules.

The separate mDNS command is candidate discovery only: it returns sanitized
cardinality and field-presence information and does not bind or select an exact
Authentics 300. Exact control still requires the configured pinned-mTLS and
UPnP model checks.

## Supported test fingerprint

- JBL Authentics 300, firmware `26.24.31.50.00`
- Harman Kardon Aura Studio 5
- protocol behavior cross-checked against JBL One Android app `2.7.9` and
  Harman Kardon One Android app `2.6.11`
- Ubuntu 22.04 / BlueZ 5.64
- open, single-subgroup request with quality `0`

Other firmware and models are unverified.

## How the historical v0.4 compatibility path works

1. Before changing either role, a lightweight local supervisor resolves the
   Aura's current random LE address from a verified live FDDF advertisement,
   connects both speakers, and keeps both control sessions open. The stable
   BR/EDR route remains a compatibility fallback.
2. The v0.4 path writes OneOS `ENTER_AURA_CAST` and
   `SET_AURACAST_BROADCAST` to the JBL
   private PL characteristic. The controller intent is to start this JBL as a
   broadcaster.
3. Linux writes AA token `0x3c=ON` to the Aura over ATT on the resolved live LE
   bearer (or the compatible stable BR/EDR fallback). The controller records
   the Aura as SECONDARY/on after a successful reply.
4. Its `stop` uses the already-open sessions in the safe order Aura OFF, JBL
   `action=2`, then JBL EXIT. It does not gamble on reconnecting after the role
   transition.
5. The observed result is consistent with device-side firmware coordination;
   the user verifies only that both speakers are audible.

The tool does not duplicate audio to two Linux sinks, which avoids the two-clock
delay seen with independent AirPlay/A2DP outputs.

## v0.5 Rust alpha quick start

First prepare the documented owner-only configuration outside the repository,
including your authorized client certificate/private key, exact device pin and
private identity anchors. This project does not distribute vendor credentials.
See the complete [Rust alpha guide](rust/README.md) before enabling writes.

```bash
./rust/build-neutral-release.sh
./scripts/install-rust-user-service.sh
# Populate and permission-check the installed owner-only config.
jbl-aura-link shutdown
systemctl --user disable --now jbl-aura-link-session.service
systemctl --user enable jbl-aura-link-rust.service
systemctl --user start jbl-aura-link-rust.service
jbl-aura-link-rust status
jbl-aura-link-rust start
jbl-aura-link-rust stop
```

The local page is `http://127.0.0.1:8096`. The tested firmware defaults to
`JBL_BROADCAST_CONFIRMATION=ack`, so a transport-accepted action reports
`accepted_unconfirmed`/`broadcast_acknowledgement_only` with CLI exit `0`; it is
not a `7951` or acoustic claim.

## v0.4 fallback quick start

Install the small runtime dependency set:

```bash
sudo apt install bluez bluez-tools jq python3 python3-venv xxd
# PulseAudio hosts also need: sudo apt install pulseaudio-utils

runtime_env="${XDG_DATA_HOME:-$HOME/.local/share}/jbl-aura-link/venv"
python3 -m venv "${runtime_env}"
"${runtime_env}/bin/pip" install -r requirements-le.txt
```

Pair and trust both speakers with `bluetoothctl`. Disconnect the Aura from any
phone or other Bluetooth host, then:

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
# Replace both placeholder addresses and point PYTHON_BIN at venv/bin/python.

./bin/jbl-aura-link doctor
./bin/jbl-aura-link install-service

jbl-aura-link status
jbl-aura-link start
```

Start audio on the JBL and listen to both speakers. To unlink:

```bash
jbl-aura-link stop
```

`stop` leaves the two control sessions ready for a later fully automatic
`start`. When deliberately handing the speakers back to an App or ending the
control session, release them and restore any previous Aura A2DP profile:

```bash
jbl-aura-link shutdown
```

Version 0.4 uses up to three 30-second live FDDF scan bursts, separated by
15-second delays. If the speaker remains between advertising windows after all
three bursts, startup fails before any role write rather than guessing from a
stale RPA. The installed systemd unit then retries after 20 seconds. Two
no-button cold starts passed on the tested pair, with one safe first-burst miss
between them. While the supervisor remains `ready` or `linked`, normal
`start`/`stop` does not rescan and remains the most reliable daily path.

The real device config lives outside the repository by default. Never put
device addresses, captures, certificates, account tokens, or app packages in
an issue or commit.

## Commands

| Command | Purpose |
|---|---|
| `doctor` | Check tools, adapter, config and pairing |
| `install-service` | Install and enable the private per-user boot service and launcher |
| `start` | Start/reuse the persistent sessions, then link |
| `stop` | Unlink through the held sessions and keep them ready |
| `shutdown` | Unlink and close both sessions, then best-effort restore prior A2DP |
| `status` | Show managed state, or conservative cached DFFD fallback |
| `recover-stop` | Explicit best-effort recovery without a held session |
| `frame` | Build PL frames offline without touching hardware |

`install-service` copies only the public CLI, session manager and unit template
under `~/.local`, installs `~/.local/bin/jbl-aura-link`, and enables
`jbl-aura-link-session.service`. At boot it establishes the control bearers and
stays `ready`; it does not link the speakers or start audio until `start` is
called. True pre-login boot startup requires user lingering; the installer
reports when it is disabled.

The daemon monitors both held control bearers. An idle disconnect or any
`degraded` command result makes the daemon exit nonzero; the installed unit then
rescans and reconnects. If the prior state could have changed speaker roles,
the replacement daemon sends the verified OFF/STOP/EXIT sequence before it
publishes `ready`.

Without an installed unit, `start` retains the transient-user-systemd fallback.
The held session path is Python standard library code; automatic LE cold
discovery adds the small `dbus-fast` dependency. The supervisor keeps both
`gatttool` children and accepts only local commands over a mode-0600 Unix socket
in a mode-0700 directory. If Linux owned the Aura A2DP profile, it remains
released while the supervisor is active. `shutdown` releases control first,
then attempts a bounded A2DP restoration; a rejected restoration is reported as
pending rather than keeping the control supervisor alive.

The systemd unit uses main-process-first termination so the supervisor gets a
bounded chance to send the safe unlink sequence before its transport children
are reaped. Power loss, speaker power-off, or an unconditional process kill
still cannot be made transactional.

On PulseAudio hosts, the default `auto` guard temporarily unloads only
`module-bluetooth-policy` and `module-bluetooth-discover` while establishing the
two control sessions, then reloads both. The verified sessions remain alive
after those modules return. A mode-0600 private restoration snapshot lets the
per-user unit repair those modules even after a failed or restarted startup.
Other Bluetooth audio is interrupted only during that short setup window.

On hardware where the FDDF/LE path has been verified, set `AURA_TRANSPORT=le`
for the boot service. `auto` retains the classic compatibility fallback, which
can also make BlueZ expose an Aura A2DP sink on some hosts.

If the supervisor is killed while linked, the speakers may reject a fresh
recovery connection. `recover-stop` is deliberately labelled best-effort and
returns nonzero rather than claiming an unlink it could not verify.

The lock lives in a user-private runtime/state directory, never as a fixed file
under `/tmp`. Direct BlueZ session release is bounded by
`BLUEZ_CONTROL_TIMEOUT` (default 5 seconds).

## Documentation

- [Reproduction guide](docs/REPRODUCTION.md)
- [No-button cold reconnect acceptance](docs/COLD_RECONNECT_2026-08-29.md)
- [Protocol notes](docs/PROTOCOL.md)
- [Evidence and unresolved questions](docs/EVIDENCE.md)
- [Official-App runtime evidence, 2026-08-30](docs/OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md)
- [Prior open-source research](docs/PRIOR_RESEARCH.md)
- [Security policy](SECURITY.md)
- [Changelog](CHANGELOG.md)

## Resource use

The implementation is Bash plus Python, `dbus-fast`, and BlueZ `gatttool`. It
has no audio-decoding, model, cloud, or account dependency.

## License and trademarks

Original code and documentation are MIT licensed. This is an independent
interoperability project, not affiliated with or endorsed by JBL, Harman Kardon,
or their owners. Product names are used only to identify compatibility. No app
or firmware binaries, decompiled source, account material, or vendor secrets are
distributed by this repository.
