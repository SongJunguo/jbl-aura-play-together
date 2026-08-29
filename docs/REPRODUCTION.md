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
sessions open. `shutdown` closes those sessions, then makes a bounded attempt to
reconnect the Aura A2DP profile only if this tool recorded that it had released
the profile. A rejected optional A2DP restoration is reported as pending but
does not keep the control supervisor alive.

## Prerequisites

The verified host used Ubuntu 22.04 and BlueZ 5.64. Install:

```bash
sudo apt install bluez bluez-tools jq python3 python3-venv xxd
# PulseAudio hosts also need: sudo apt install pulseaudio-utils

runtime_env="${XDG_DATA_HOME:-$HOME/.local/share}/jbl-aura-link/venv"
python3 -m venv "${runtime_env}"
"${runtime_env}/bin/pip" install -r requirements-le.txt
```

Pair and trust each speaker once through `bluetoothctl`. The JBL must expose its
public LE identity and the Aura its stable BR/EDR identity. Install the private
configuration outside the repository:

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
```

Replace only the two placeholder addresses, and set `PYTHON_BIN` to the venv's
`bin/python`. Keep the file mode at `0600`.

## Install the boot service

After `doctor` passes, install the per-user service and simple launcher:

```bash
./bin/jbl-aura-link doctor
./bin/jbl-aura-link install-service
jbl-aura-link status
```

The unit is enabled under the user `default.target`. It starts the held control
session into `ready` but does not enable Play Together or start audio. On hosts
where `loginctl show-user "$USER" -p Linger` reports `yes`, the user manager and
service start at boot without an interactive login. Otherwise they start at
login; the installer prints the exact `loginctl enable-linger` command needed
for true boot startup.

The unit uses `Restart=on-failure`. Each daemon attempt makes at most three
30-second verified FDDF scans separated by 15 seconds, and systemd waits another
20 seconds before restarting a failed attempt. PulseAudio restoration state is
kept outside the repository with mode `0600` so a failed startup cannot silently
leave the two Bluetooth modules unloaded.

For the verified Aura Studio 5 path, set this after the first successful LE
acceptance:

```ini
AURA_TRANSPORT=le
```

This prevents the boot service from falling back to the stable BR/EDR control
bearer, which can also cause BlueZ/PulseAudio to expose an Aura A2DP sink. Keep
`auto` only when classic compatibility is more important than excluding that
host-side sink.

The daemon treats an idle control-bearer disconnect or a degraded command as a
service failure. systemd reconnects it, and an uncertain prior role state is
normalized with Aura OFF, JBL STOP and JBL EXIT before `ready` becomes visible.

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

With the installed service, the boot unit supplies the supervisor. Without it,
first start creates a transient local supervisor. The
expected CLI result says the persistent session is ready and the managed link
sequence completed; `status` should report `linked`. On PulseAudio, the two
Bluetooth modules may be briefly suspended while both control bearers connect,
then restored before `start` returns.

In default `auto` mode the cold path first scans typed BlueZ D-Bus events for a
random-address advertisement whose FDDF UUID, product ID, and embedded stable
identity all match the configured Aura. `status` reports `Aura transport: le`
when that path supplied the held control session.

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

For a useful held-session repeatability check, complete at least two
`start`/`stop` cycles through that same supervisor. The earlier v0.3 hardware
run completed four starts and three stops. To test the separate cold path, run:

```bash
./bin/jbl-aura-link shutdown
./bin/jbl-aura-link status
```

After `shutdown`, the supervisor is offline; optional A2DP restoration happens
only after the raw LE control bearer closes. A later `start` must build fresh
sessions. Two no-button cold rounds passed on the tested pair. One intervening
30-second scan received advertisements but no Aura FDDF and failed safely before
any command; retrying later passed. Do not use `shutdown` between ordinary daily
start/stop cycles when the lower-latency held session is acceptable.

## Exceptional recovery

If an App, an older one-shot script, power loss, or a killed supervisor leaves a
role-changed speaker rejecting fresh connections, no program can send a
guaranteed OFF command over a connection it cannot establish.

- If `status` still shows a managed session, use `stop` or `shutdown` first.
- If the local session is gone, ordinary `stop` deliberately fails instead of
  pretending it unlinked anything.
- `recover-stop` exposes the old fresh-connection method explicitly as
  best-effort and returns nonzero when acknowledgements are incomplete.
- If cold `start` reports that no verified FDDF advertisement appeared, keep the
  Aura powered, confirm other hosts are disconnected, and retry after its
  advertising state has had time to resume. Do not persist the reported RPA or
  substitute a same-name device.
- A physical Bluetooth-button press remains a compatibility fallback for the
  older BR/EDR route, but it is no longer part of the normal verified LE cold
  procedure.

Each FDDF scan burst is 30 seconds. The tested pair produced successful matches
after about 10.7 seconds in two passive probes, but also produced one complete
30-second identity-data gap. Version 0.4 adds two delayed scan retries and the
installed unit adds restart-on-failure; this still does not justify a fixed
maximum reconnect-time promise from the small hardware sample.

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
