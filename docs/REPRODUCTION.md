# Reproduction guide

This guide separates three different claims: transport delivery, managed
start/stop repeatability, and two-speaker audibility. A successful local command
does not by itself prove BASS, BIG/BIS, ISO audio, or exact synchronization.

## Safety and scope

- Begin at a barely audible volume and use a short, non-sensitive source.
- Stop phone apps and other programs that control either speaker.
- Disconnect the Aura from phones and other Bluetooth hosts without deleting
  its pairing. The tested state did not accept Ubuntu's classic control link
  while a phone held it.
- Do not publish real configs, process listings, raw Bluetooth captures, APKs,
  account material, network addresses, or device identifiers.
- The CLI does not start music. It only changes the two speaker roles.

`stop` reverses the vendor association but intentionally keeps both control
sessions open. `shutdown` closes those sessions and reconnects the Aura A2DP
profile only if this tool recorded that it had released the profile.

## Prerequisites

The verified host used Ubuntu 22.04 and BlueZ 5.64. Install:

```bash
sudo apt install bluez bluez-tools jq python3 xxd
# PulseAudio hosts also need: sudo apt install pulseaudio-utils
```

Pair and trust each speaker once through `bluetoothctl`. The JBL must expose its
public LE identity and the Aura its stable BR/EDR identity. Install the private
configuration outside the repository:

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
```

Replace only the two placeholder addresses. Keep the file mode at `0600`.

## Preflight and negative control

1. Power on both speakers and wait for normal idle state.
2. Disconnect the Aura from the phone; do not open either vendor App during the
   trial.
3. Run `./bin/jbl-aura-link doctor` and resolve every blocking result.
4. Start a short, low-volume source through a method the JBL already supports.
5. Confirm the JBL is audible and the Aura is silent.

Do not treat the trial as a new link result if both speakers already play at
baseline; a previous controller may still own the association.

## Managed link and unlink trial

Run:

```bash
./bin/jbl-aura-link start
./bin/jbl-aura-link status
```

On a systemd-user host, first start creates a transient local supervisor. The
expected CLI result says the persistent session is ready and the managed link
sequence completed; `status` should report `linked`. On PulseAudio, the two
Bluetooth modules may be briefly suspended while both control bearers connect,
then restored before `start` returns.

Continue the same JBL source and listen to each physical speaker. Record the
acoustic pass only when:

- the JBL remains audible;
- the Aura changes from silent to audible;
- no independent Linux stream targets the Aura;
- both reproduce the same content transition.

Then run:

```bash
./bin/jbl-aura-link stop
./bin/jbl-aura-link status
```

The expected managed state is `ready`: JBL remains available for its source,
Aura becomes silent, and both control bearers stay open. A second `start` should
reuse them without an App, button press, re-pair, or Bluetooth reconnect.

For a useful repeatability check, complete at least three `start`/`stop` cycles
through that same supervisor. The verified v0.3 run completed four starts and
three stops, followed by:

```bash
./bin/jbl-aura-link shutdown
./bin/jbl-aura-link status
```

After `shutdown`, the supervisor is offline and any recorded Aura A2DP profile
is restored when the speaker accepts it. A later `start` must build fresh
sessions. One immediate cold rebuild passed, but another attempt after the
Aura's connectable window closed failed before command delivery. Do not use
`shutdown` between ordinary automatic start/stop cycles.

## Exceptional recovery

If an App, an older one-shot script, power loss, or a killed supervisor leaves a
role-changed speaker rejecting fresh connections, no program can send a
guaranteed OFF command over a connection it cannot establish.

- If `status` still shows a managed session, use `stop` or `shutdown` first.
- If the local session is gone, ordinary `stop` deliberately fails instead of
  pretending it unlinked anything.
- `recover-stop` exposes the old fresh-connection method explicitly as
  best-effort and returns nonzero when acknowledgements are incomplete.
- On the tested stale state, pressing the Aura Bluetooth button once restored a
  connectable state. The following held-session start/stop sequence required no
  further press.
- After a true `shutdown`, the next `start` may require that one physical press
  again. No matching always-on LE fallback was visible in the tested closed
  window.

When that press is required, run `start` first. The default supervisor retries
the Aura bearer every 250 ms for 45 seconds, so press the Bluetooth button while
the command is waiting. Do not press before starting a slow chain of setup
commands; the observed blue-light interval lasts only a few seconds.

Avoid repeatedly pressing buttons while the supervisor is healthy; that changes
the state being tested. If a phone App is needed again, run `shutdown` before
opening it.

## Interpreting status

While the supervisor is active, `status` reports its local state machine:
`ready`, `linked`, or `degraded`. Those values summarize control
acknowledgements only. They are not BASS or ISO observations.

When the supervisor is offline, `status` can show a cached JBL DFFD role. Its
freshness is unknown, and its private `RECEIVER(2)` enum is unrelated to the
OneOS status namespace where raw `2` means broadcaster. Treat the offline
association state as unknown unless verified acoustically.

## Sanitized repeatability record

Record only:

- speaker models and firmware versions;
- Linux distribution, kernel, BlueZ, and tool version;
- managed state transitions and which application-level acknowledgements were
  observed;
- the JBL/Aura audible yes/no result at baseline, link, and unlink;
- whether a graceful shutdown, host restart, or speaker power cycle changed the
  outcome.

Replace every Bluetooth and network address with a role label before sharing.
Do not interpret a protected or failed BASS read as an empty Receive State.

## Stronger protocol evidence

Identifying the actual broadcaster requires a fresh HCI/ISO-capable capture or
a passive LE Audio sniffer that sees periodic advertising, BASE, BIGInfo, and
BIG/BIS. This repository has not made that capture. Its verified claim stops at
the vendor control sequence, repeatable held-session lifecycle, and separately
observed two-speaker audibility.
