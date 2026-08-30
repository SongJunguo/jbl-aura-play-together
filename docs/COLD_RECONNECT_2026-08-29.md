# No-button Aura cold reconnect acceptance, 2026-08-29

This is the sanitized hardware record for the first complete Linux cold-control
path that did not require an Aura Studio 5 Bluetooth-button press. It records a
small bounded experiment, including the failed attempt. It is not a statistical
reliability claim.

## Scope and starting state

- Host: Ubuntu 22.04, BlueZ 5.64.
- Speakers: JBL Authentics 300 and Harman Kardon Aura Studio 5.
- The Aura remained powered on.
- Android's system Bluetooth UI disconnected the Aura. The phone Bluetooth
  radio remained enabled for an unrelated headset; the whole phone radio was
  not disabled.
- Android diagnostics reported Aura A2DP disconnected and no active Harman GATT
  client.
- No vendor App action or physical speaker-button press was used in the clean
  trials.
- Music playback was stopped except for the one explicit acoustic check below.

## Identity and connection rule

The Aura control address is a rotating random LE address and is not persisted.
The resolver applies a BlueZ D-Bus discovery filter with:

```text
Transport     = le
DuplicateData = true
Pattern       = ""
```

It accepts a fresh `org.bluez.Device1` event only when all of these are true:

1. `AddressType` is `random`;
2. Service Data contains the SIG-assigned Harman UUID `0xFDDF`;
3. payload bytes 0-1 equal Aura Studio 5 PID `212d` in little-endian order;
4. payload bytes 11-16 equal the privately configured stable BR/EDR identity.

The matching live address is handed immediately to a random-address ATT
session. The session enables notifications at the verified CCCD, then reuses
the already established Aura AA command/acknowledgement path. Addresses are
redacted from diagnostics and are not written to the public repository.

## Hardware observations

| Step | Result | Sanitized evidence |
|---|---|---|
| Long read-only identity probe | Pass | resolved after 10.7 s; 21 advertisement events, 1 FDDF event, 1 identity match |
| Clean managed cold start | Pass | managed state `linked`; Aura transport `le`; PulseAudio modules restored |
| Acoustic check | Pass | one source sent only to JBL at 15%; listener explicitly confirmed both speakers audible |
| Cold-control round 1 | Pass | `shutdown -> start -> stop`; ended `ready/le`; 17 s |
| Immediate next cold attempt | Safe fail | 30 s; 49 advertisement events, 0 FDDF, 0 identity matches; no role command sent |
| Later read-only identity probe | Pass | resolved after 10.7 s; 22 events, 1 FDDF event, 1 identity match |
| Cold-control round 2 | Pass | cold `start -> stop`; ended `ready/le`; 15 s |

The user requested stopping after two successful cold-control rounds, so no
additional hardware cycles were run.

## What the failed attempt means

The failed scan was not a transport write failure and did not corrupt the
working role state. BlueZ was receiving other BLE events, but the Aura did not
expose a matching FDDF payload during that window. The resolver refused to use
a same-name device or stale random address, restored the temporarily suspended
PulseAudio modules, and exited before all vendor role writes.

FDDF appeared again later. The current evidence therefore supports:

- no-button cold reconnect is genuinely possible on this pair;
- the PID/stable-identity matcher reaches the correct live control bearer;
- the Aura has an advertising window, cooldown, or duty-cycle behavior that is
  not yet characterized;
- two successful rounds do not justify a 100% first-attempt claim.

For daily use, `stop` should leave the supervisor in `ready`. A later `start`
then reuses the held sessions and avoids cold discovery entirely.

## Shutdown ordering result

An older candidate tried to restore the Aura's classic A2DP profile before
closing its raw LE vendor-control bearer. BlueZ returned
`br-connection-busy`, so this ordering could retain the supervisor indefinitely.

After the LE bearer was closed, the busy error disappeared. The speaker could
still reject the optional classic connection with `Host is down`, which is a
separate BR/EDR availability issue and is not required for JBL-origin
Play Together audio.

Version 0.4 therefore defines graceful shutdown as:

```text
verified STOP
  -> close both vendor control sessions
  -> supervisor offline
  -> bounded best-effort restoration of a previously released Aura A2DP profile
```

If that optional restoration is rejected, the private restore marker is kept
and a warning is printed. The control shutdown itself is not rolled back.

## Final state after the acceptance pass

- managed supervisor: `ready`;
- Aura control transport: `le`;
- Play Together: stopped;
- test music: stopped;
- both PulseAudio Bluetooth modules: present;
- public code: v0.4 public release; no private artifacts included.

The offline resolver, wrapper, persistent-session, failure-injection,
ShellCheck, privacy, and diff-format checks all passed after the hardware run.

## v0.4 service hardening and live recovery

The boot-service deployment exposed two additional lifecycle cases and retained
them as regression tests:

1. The first installer revision held its operation lock while starting the new
   unit. `ExecStartPre` correctly rejected that concurrent operation and its
   cleanup hook restored both PulseAudio modules. The enabled unit retried after
   20 seconds and reached `ready/le`. The installer now releases its lock before
   asking systemd to start the unit.
2. An Aura LE bearer later disconnected while the process still reported
   `ready`. The first command detected `Disconnected` and moved to `degraded`.
   Version 0.4 now monitors both held bearers, exits nonzero on idle loss or a
   degraded command, and lets systemd reconnect. A prior uncertain state is
   normalized with Aura OFF, JBL STOP and JBL EXIT before publishing `ready`.

The delayed retry was then exercised on real hardware rather than only through
fixtures. One service restart missed FDDF for its first 30-second scan, waited
15 seconds, matched a later advertisement, normalized the prior state, and
reached `ready/le`. A deliberate termination of only the Aura control child
produced `exit-code`, triggered the unit's 20-second restart, missed the first
FDDF scan again, then recovered on the delayed retry. After recovery:

- the installed `start -> status -> stop -> status` sequence passed;
- final managed state was `ready` over `le`;
- both PulseAudio Bluetooth modules were present;
- the private PulseAudio restoration snapshot was clear;
- no Linux Aura A2DP sink was present;
- no music was played during these service tests.

The tested host pins `AURA_TRANSPORT=le` for the boot service. `auto` remains a
public compatibility option, but its classic fallback caused BlueZ to expose an
idle Aura A2DP sink on this host and was therefore not retained as the deployed
audio-routing state.

## 2026-08-30 Rust native follow-up

The Rust v0.5 whole-pair backend was later exercised against the same device
pair. This follow-up preserves the 2026-08-29 v0.4 evidence above as historical
evidence rather than merging the two implementations into one result.

The first real Rust `stop` occurred during an FDDF advertising window in which
the Aura identity could not be proved. The action was rejected before the first
device write and the write-ahead journal returned to `clean`.

A later direct connection attempt to the discovered LE `Device1` failed. A
bounded nudge through the paired-and-trusted stable public object caused BlueZ
to connect, but the vendor GATT service appeared on the unique connected random
object. Rust adopted that object only after its FDDF payload exactly matched the
expected PID and embedded stable identity. The stable object was therefore a
connection trigger, not substitute identity evidence.

An explicit Rust recovery then:

1. performed the safe diagnostic checks required by the pending/failed state;
2. verified the paired stable Aura identity;
3. mapped that identity to the exact current random GATT identity from fresh
   FDDF service data;
4. obtained the required acknowledgements and returned managed state to
   `ready`.

The native Rust path subsequently completed accepted `start` and `stop`
actions. Two no-button cold `start` rounds passed; managed status reported
`br_edr` during the first and ended `le` during the second. Two normal STOP
actions through the retained session completed in approximately 0.44 and 0.57
seconds. These are bounded functional results for the tested pair, not evidence
of a 100% first-attempt rate or permission to skip the random-object identity
gate.

The crash boundary was also observed directly. An older timeout construction
caused a real process panic after the pending journal record had been durably
written. Restart preserved that pending record and blocked ordinary mutation.
The timeout construction was fixed; explicit accepted recovery, rather than a
restart or backend switch, was still required to clear uncertainty.

The approximately 03:45 full-song Rust attempt was later found to overlap an
automatic Home Centre STOP. Its JBL-only audio is therefore contaminated and
cannot refute or validate a protocol transaction. A later clean trial used the
current default transaction without the separate Assistant command `7957`:
Aura AA ON and JBL Wi-Fi ENTER were accepted and managed state became `linked`.
After the EOF fix, waiting `15` seconds before JBL-only network playback still
produced no Aura audio. The official Home flow's successful direction was
instead Android A2DP into Aura, with Aura PRIMARY and JBL RECEIVER. The project
target therefore reintroduces separate Assistant `7957` JBL-broadcaster
semantics as a cross-state-machine composition. Exact GATT `0x002a` then
produced the first Rust target-direction START acoustic pass at requested `5%`.
No `7951` was observed; ordinary STOP failed on an Aura ACK timeout and explicit
recovery returned ready within `13` seconds. After the fresh-bearer fix, round
two repeated the dual-audio START and completed one ordinary approximately
`43`-second STOP accepted/ready without recovery. This makes no
BASS/BASE/BIG/BIS/ISO claim. Production wake is present and offline-green, but
the latest no-button cold hardware round passed via `fresh_le` inside `150`
seconds. A2DP `wake_then_stable`, `7951` and P0/release remain incomplete. The
Rust controller does not automatically fail over to v0.4 or replay a write after
failure; v0.4 remains a separately selected fallback whose historical
compatibility path includes `7957`.

## Relevant independent references

- [partybox-companion](https://github.com/jklingberg/partybox-companion), which
  independently documents rotating Harman control addresses, the FDDF stable
  identity offset, and live-scan connection handling;
- [BlueZ 5.64 Adapter API](https://github.com/bluez/bluez/blob/5.64/doc/adapter-api.txt),
  for typed discovery filters and non-discoverable device objects;
- [Bleak rotating-address discussion](https://github.com/hbldh/bleak/discussions/1246),
  for scan/match/connect guidance rather than persisted RPAs;
- [BlueZ issue 2356](https://github.com/bluez/bluez/issues/2356), showing that
  active-scan-then-connect can be required even on newer BlueZ;
- [Harman multi-speaker patent WO2025081468A1](https://patents.google.com/patent/WO2025081468A1/en),
  used only as architecture-level corroboration.

See [Protocol](PROTOCOL.md), [Evidence](EVIDENCE.md), and
[Prior research](PRIOR_RESEARCH.md) for the broader evidence boundary. No fresh
JBL `7951`, BASS Receive State, BASE, BIGInfo, BIS/ISO capture, or measured
acoustic offset was produced by this acceptance pass.
