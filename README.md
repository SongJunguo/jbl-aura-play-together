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

The official PartyTogether UI uses the ENTER/EXIT state machine, while `7957`
belongs to a separate broadcaster-assistant path inside the controller. This
project combines controller-supported semantics from those two paths; it does
not claim that its transaction is a byte-for-byte replay of one official UI
transaction.

The same day exposed and resolved an important Linux lifecycle problem. A
one-shot `start` succeeded, but an immediate one-shot `stop` could not create
fresh control connections after the speakers changed role. Keeping both
control sessions open fixed that failure: three START operations and two STOP
operations then completed consecutively without another button press, App,
re-pair, or reconnect. Every Aura ON/OFF returned the App-defined
`aa00021300` success reply; JBL ENTER/EXIT returned `error_code=0`.

The automated v0.3 supervisor subsequently completed four START operations and
three STOP operations without another button press. The user-systemd service
survived across CLI invocations and both PulseAudio Bluetooth modules were
restored. One immediate shutdown/rebuild also succeeded, but a later rebuild
after the Aura's connectable window had closed failed before any command with
`Host is down`. This defines the honest automation boundary: managed
start/stop is repeatable while the sessions are held; a later cold session may
need one Aura Bluetooth-button press. No playback was started in this lifecycle
pass, so it is not presented as another acoustic or BASS result.

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

## Supported test fingerprint

- JBL Authentics 300, firmware `26.24.31.50.00`
- Harman Kardon Aura Studio 5
- protocol behavior cross-checked against JBL One Android app `2.7.9` and
  Harman Kardon One Android app `2.6.11`
- Ubuntu 22.04 / BlueZ 5.64
- open, single-subgroup request with quality `0`

Other firmware and models are unverified.

## How it works

1. Before changing either role, a lightweight local supervisor connects to the
   JBL and Aura and keeps both control sessions open.
2. Linux writes OneOS `ENTER_AURA_CAST` and `SET_AURACAST_BROADCAST` to the JBL
   private PL characteristic. The controller intent is to start this JBL as a
   broadcaster.
3. Linux writes AA token `0x3c=ON` to the Aura over ATT on its stable classic
   Bluetooth identity. The controller records the Aura as SECONDARY/on after a
   successful reply.
4. `stop` uses the already-open sessions in the safe order Aura OFF, JBL
   `action=2`, then JBL EXIT. It does not gamble on reconnecting after the role
   transition.
5. The observed result is consistent with device-side firmware coordination;
   the user verifies only that both speakers are audible.

The tool does not duplicate audio to two Linux sinks, which avoids the two-clock
delay seen with independent AirPlay/A2DP outputs.

## Quick start

Install the small runtime dependency set:

```bash
sudo apt install bluez bluez-tools jq python3 xxd
# PulseAudio hosts also need: sudo apt install pulseaudio-utils
```

Pair and trust both speakers with `bluetoothctl`. Disconnect the Aura from any
phone or other Bluetooth host, then:

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
# Replace both placeholder MAC addresses in "${config_path}".

./bin/jbl-aura-link doctor
./bin/jbl-aura-link start
./bin/jbl-aura-link status
```

Start audio on the JBL and listen to both speakers. To unlink:

```bash
./bin/jbl-aura-link stop
```

`stop` leaves the two control sessions ready for a later fully automatic
`start`. When deliberately handing the speakers back to an App or ending the
control session, release them and restore any previous Aura A2DP profile:

```bash
./bin/jbl-aura-link shutdown
```

The Aura may close its classic connectable window after `shutdown`. If the
first managed `start` reports `Host is down`, press its Bluetooth button once
and retry. No further press was needed during the verified held-session
`start`/`stop` cycles. Prefer `stop`, not `shutdown`, when later automatic
restart matters.

Version 0.3 waits for the scarce Aura bearer and retries it every 250 ms for up
to 45 seconds by default. For a closed window, run `start` first and press the
Aura Bluetooth button while that command is waiting; this avoids racing the
two-to-three-second blue-light interval.

The real device config lives outside the repository by default. Never put
device addresses, captures, certificates, account tokens, or app packages in
an issue or commit.

## Commands

| Command | Purpose |
|---|---|
| `doctor` | Check tools, adapter, config and pairing |
| `start` | Start/reuse the persistent sessions, then link |
| `stop` | Unlink through the held sessions and keep them ready |
| `shutdown` | Unlink, close both sessions, and restore prior A2DP |
| `status` | Show managed state, or conservative cached DFFD fallback |
| `recover-stop` | Explicit best-effort recovery without a held session |
| `frame` | Build PL frames offline without touching hardware |

`start` launches a transient user-systemd supervisor when available. The
supervisor is Python standard library only, keeps both `gatttool` children, and
accepts only local commands over a mode-0600 Unix socket in a mode-0700
directory. If Linux owned the Aura A2DP profile, it remains released while the
supervisor is active and `shutdown` restores it.

The systemd unit uses main-process-first termination so the supervisor gets a
bounded chance to send the safe unlink sequence before its transport children
are reaped. Power loss, speaker power-off, or an unconditional process kill
still cannot be made transactional.

On PulseAudio hosts, the default `auto` guard temporarily unloads only
`module-bluetooth-policy` and `module-bluetooth-discover` while establishing the
two control sessions, then reloads both. The verified sessions remain alive
after those modules return. Other Bluetooth audio is interrupted only during
that short setup window.

If the supervisor is killed while linked, the speakers may reject a fresh
recovery connection. `recover-stop` is deliberately labelled best-effort and
returns nonzero rather than claiming an unlink it could not verify.

The lock lives in a user-private runtime/state directory, never as a fixed file
under `/tmp`. Direct BlueZ session release is bounded by
`BLUEZ_CONTROL_TIMEOUT` (default 5 seconds).

## Documentation

- [Reproduction guide](docs/REPRODUCTION.md)
- [Protocol notes](docs/PROTOCOL.md)
- [Evidence and unresolved questions](docs/EVIDENCE.md)
- [Prior open-source research](docs/PRIOR_RESEARCH.md)
- [Security policy](SECURITY.md)

## Resource use

The implementation is Bash plus one Python-standard-library supervisor around
BlueZ `gatttool`. It has no CUDA, GPU, audio decoding, model, cloud, or account
dependency.

## License and trademarks

Original code and documentation are MIT licensed. This is an independent
interoperability project, not affiliated with or endorsed by JBL, Harman Kardon,
or their owners. Product names are used only to identify compatibility. No app
or firmware binaries, decompiled source, account material, or vendor secrets are
distributed by this repository.
