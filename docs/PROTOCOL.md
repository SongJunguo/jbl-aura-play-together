# Protocol notes

These notes describe interoperability observed on one tested firmware pair.
They are not an official JBL or Harman Kardon specification.

Evidence labels used below:

- **Observed**: captured from the local device or transport.
- **Decompiled**: reconstructed as interoperability facts from data structures
  in JBL One 2.7.9 or Harman Kardon One 2.6.11. No app source is distributed.
- **Verified**: replayed on real hardware with an externally visible result.
- **Inferred**: a plausible explanation that remains unproven.

## JBL OneOS PL transport

Tested transport parameters:

| Item | Value |
|---|---|
| Address type | public LE identity |
| Write handle | `0x002a` |
| Negotiated MTU | `500` |
| Magic | `50 4c` (`PL`) |

Frame layout:

```text
50 4c | command_id uint16-le | UTF-8 payload length uint16-le | payload
```

Relevant commands recovered from `EnumCommandMapping`:

| Decimal | Hex | Name |
|---:|---:|---|
| 7937 | `0x1f01` | `ENTER_AURA_CAST` |
| 7938 | `0x1f02` | `EXIT_AURA_CAST` |
| 7942 | `0x1f06` | full OneOS `SET_AURA_CAST_GROUP` |
| 7943 | `0x1f07` | `DESTROY_AURA_CAST_GROUP` |
| 7944-7949 | `0x1f08`-`0x1f0d` | group info, parameters, rename, notifications and channel switch |
| 7950 | `0x1f0e` | `GET_DEVICE_AURACAST_BROADCAST_INFO` |
| 7951 | `0x1f0f` | broadcast-info notification |
| 7952-7954 | `0x1f10`-`0x1f12` | scan and scan-state operations |
| 7955 | `0x1f13` | connect a OneOS receiver to a broadcast |
| 7956 | `0x1f14` | switch subgroup |
| 7957 | `0x1f15` | `SET_AURACAST_BROADCAST` |

The start request uses this schema. The address below is a locally administered
placeholder, not a real device address:

```json
{
  "action": 1,
  "broadcast": {
    "address": "02:00:00:00:00:01",
    "name": "JBL Authentics 300",
    "need_access_code": false,
    "status": 2,
    "subgroup": [{
      "active_status": 1,
      "index": 0,
      "is_support": true,
      "quality": 0
    }]
  }
}
```

Gson omitted null fields in the official app. The no-auth request therefore
does not include `access_code` or a null subgroup name. Stop is `{"action":2}`.

The JSON value `status: 2` belongs to OneOS
`DeviceAuracastBroadcastInfo`, where `2` means broadcaster. It is not the same
enum as the DFFD `OneAuracastRole` described below, where raw value `2` means
receiver. Comparing those two raw integers directly would be an error.

JBL One's `7957` assistant path fills the broadcast address from the controlled
device's own address, upper-cases it, marks one subgroup active, and waits for
the `7951` broadcast-info notification before reporting application-level
success. Its 15-second result path treats action code `33` as start success and
`31` as start failure; stop uses `34` and `32`, respectively. Command `7950`
queries current broadcast information. This is distinct from `7942`: the
full-group helper sends a shared, named membership object to multiple selected
OneOS devices and waits for all of them to enter group mode. The historical
v0.4 Authentics-plus-Aura compatibility sequence uses `7957` on the JBL and the
separate AA command below on the Aura. The Home-flow-only Rust design omitted
`7957`, but its target-direction acoustic gate is now refuted.

Dynamic controller inspection adds an important provenance boundary. JBL One's
visible PartyTogether activity uses the ENTER/EXIT state machine and persists
the selected Aura in a receiver slot. The `7957` request builder belongs to a
separate broadcaster-assistant flow. The historical v0.4 Linux sequence
combines these two controller-supported semantics; it is not claimed to be a
packet-for-packet recording of one official PartyTogether UI transaction. The
simultaneous official-App capture cross-identifies the home-flow JBL stage as
Wi-Fi `enterAuracast` and removal as Wi-Fi `exitAuracast`, with no `7957`. The
TLS plaintext was not decrypted; the conclusion combines UI timing, record
direction/length and the exact static call graph. That run sent Android A2DP
into Aura; playback made Aura PRIMARY and JBL RECEIVER. It therefore proves the
Aura-source Home direction, not the project's JBL-network-source direction.

**Important:** in the historical v0.4 test, the JBL acknowledged the `7957` ATT
write, but no `7951` application-level success notification was captured. That
is distinct from the official-App `7937` GATT attempt, which the simultaneous
capture shows was rejected as Write Not Permitted. A write ACK is necessary,
not sufficient, evidence.

## JBL OneOS HTTPS command mappings

For the tested Authentics 300 firmware, a read-only capability and App-routing
cross-check selects direct mTLS HTTPS rather than port 8080 as the primary Rust
JBL transport. The independently mapped requests are form-encoded POSTs to
`httpapi.asp`:

```text
command=enterAuracast&payload=null
command=setAuracastBroadcast&payload={"action":1,"broadcast":{...}}
command=setAuracastBroadcast&payload={"action":2}
command=exitAuracast&payload=null
```

The refuted Home-flow-only Rust design used `enterAuracast` and `exitAuracast`
without `setAuracastBroadcast`. After its EOF fix, accepted/local `linked` plus
a `15`-second pre-play wait still produced JBL-only audio when the network
source entered JBL. The target-direction implementation therefore reintroduces
the separate official Assistant broadcaster semantics alongside Aura AA
receiver semantics. This is a composition across two official state machines,
not one captured UI sequence.

The official Wi-Fi session maps PL command `7957` to
`setAuracastBroadcast` and concatenates the compact JSON literally after
`payload=`. The start object is the exact schema above with the runtime JBL
identity; logs, tests and documentation must use placeholders instead.

The response is a BasicResponse. Only `error_code == 0` establishes command
acceptance. It does not establish that broadcasting started, that the Aura
subscribed, or that either speaker is audible. The App's separate Assistant
result path waits for `7951`, which has not been captured in the Linux trials. A later bounded
`getAuraCastGroupInfo` read establishes the intended membership configuration,
not a live broadcast/receive postcondition. A controlled STOP comparison left
the same two members and `disabled=false` unchanged for at least 15 seconds.
See [membership semantics](GROUP_MEMBERSHIP_SEMANTICS_2026-08-30.md). The
official controller does not wrap these commands in `sendAppController`, whose
request model is for key presses.

On the tested firmware, the two `7957` transports are not equivalent. After
certificate-pin matching, HTTPS `setAuracastBroadcast` returned HTTP `200` with
an `unknown command` device result. Exact GATT value handle `0x002a` accepted
the `7957` write at ATT level. Four JBL GATT writes in the bounded START/STOP
exercise received ACKs, but no `7951` arrived. After an authorized narrow
firewall rule was installed, strict SUBSCRIBE-before-GATT production START still
ended `jbl_broadcast_result_timed_out` and the pair was normalized through the
legacy GATT path. The rule is not protocol-success evidence.

`JBL_BROADCAST_CONFIRMATION` is a closed `ack|gena` setting and defaults to
`ack` for this exact firmware. ACK mode reports `accepted_unconfirmed` plus
`broadcast_acknowledgement_only` and CLI exit `0`. Strict GENA reports
`accepted` plus `broadcast_business_notification` only for matching action
`33`/`34`; timeout is failure. Managed `linked` is neither a `7951` nor an
acoustic assertion.

The first exact-GATT START was accepted and a JBL-only Music Assistant source
at requested `5%` was confirmed audible on both speakers. Ordinary STOP then
ended outcome-unknown with `failure=aura_ack_timeout`; explicit recovery
returned accepted/`ready` within `13` seconds. After the fresh-bearer release
fix, round two repeated the accepted requested-`5%` dual-audio START and then,
with playback idle, completed ordinary STOP as accepted/`ready` in approximately
`43` seconds without recovery. This completes the two requested acoustic rounds
and one post-fix normal STOP, not the full P0/release lifecycle; `7951` and the
unhit A2DP `wake_then_stable` subpath remain unresolved.

The current device did not advertise `websocket_connect` and refused port
8080, so WebSocket remains only an optional transport for other firmware. The
inspected App build's optional URL is `ws://`, not evidence for WSS. See the
[control-channel decision](JBL_CONTROL_CHANNEL_2026-08-30.md).

## Aura AA transport

The Aura path is the Excelpoint/JBL AA framing used by multiple speaker lines:

```text
aa | one-byte command | one-byte payload length | payload
```

Tested Aura transport:

| Item | Value |
|---|---:|
| Link | ATT over the stable BR/EDR identity |
| PSM | `31` (`0x001f`) |
| Vendor service UUID | `65786365-6c70-6f69-6e74-2e636f6d0000` |
| Notify UUID | `65786365-6c70-6f69-6e74-2e636f6d0001` |
| Write UUID | `65786365-6c70-6f69-6e74-2e636f6d0002` |
| Write handle | `0x03ea` |
| Notify handle | `0x03ec` |
| Notify CCCD | `0x03ed` |

Play Together frames:

```text
aa 13 04 00 3c 01 01   ON
aa 13 04 00 3c 01 00   OFF
```

The current Harman Kardon One controller constructs these bytes through a
dedicated `AuraCastCommand`. Its AA serializer and status enum give the exact
decode:

| Bytes | Meaning |
|---|---|
| `aa` | AA frame identifier |
| `13` | Set Device Info command |
| `04` | four-byte payload |
| `00` | reserved byte prepended by Set Device Info |
| `3c` | AuraCast status token |
| `01` | one-byte TLV value |
| `01` / `00` | AuraCast Party ON / OFF |

The successful hardware run observed response `aa 00 02 13 00`. The app's Set
Device Info response predicate requires response command `0x00`, original
command `0x13`, and status `0x00`; the observed frame is therefore an exact
success acknowledgement. Subsequent device-info values corresponded to
SECONDARY and Play Together ON. In the controller state machine, ON updates both
general-role fields to SECONDARY; OFF restores them to NORMAL.

The official app prefers its BR/EDR control session and uses a random LE address
only as a fallback. ATT PSM 31 lets the compatibility backend use the stable
classic identity. A private Android GATT observation independently mapped the
same service to the UUIDs above, including subscription to the notify
characteristic before its five-byte acknowledgement arrived. The native Rust
BlueZ backend can therefore discover by UUID after fresh FDDF identity matching
instead of hard-coding handles or a rotating address.

### Rotating LE cold identity

A later phone HCI capture and a typed BlueZ D-Bus probe established the Aura's
fallback identity without relying on its temporary RPA:

| Field | Observed meaning |
|---|---|
| Advertising address type | random, connectable and rotating |
| Service Data UUID | `0xFDDF`, assigned by Bluetooth SIG to Harman International |
| Payload bytes 0-1 | PID `212d`, little-endian on air as `2d 21` |
| Payload bytes 11-16 | stable BR/EDR address in display order |
| Scan response | `Aura Studio 5` |

The address in the advertisement is only a live transport handle. Identity is
the conjunction of FDDF UUID, PID, and embedded stable address. The resolver
therefore calls BlueZ `SetDiscoveryFilter`, consumes typed
`InterfacesAdded`/`PropertiesChanged` data, and connects immediately with the
fresh random address. It does not persist the RPA or parse `bluetoothctl` PTY
output.

`FDDF` and the Authentics `DFFD` advertisement discussed later are distinct
app-recognized families; they must not be conflated as alternate byte-order
spellings. Although the `0xFDDF` number is SIG-assigned to Harman, the payload
schema above is still vendor-defined and undocumented.

An independent PartyBox 520 implementation reports the same FDDF stable-address
offset and the same live-scan requirement. On this Aura, a 2026-08-29 read-only
D-Bus probe resolved one verified RPA from five fresh advertisement events.
When the phone subsequently held Aura A2DP, the Aura stopped exposing FDDF and
both the typed probe and an interactive BlueZ scan correctly returned no Aura
candidate. This is a competing-host state, not a reason to accept a stale RPA.

After Android's system Bluetooth UI released the Aura, a fresh resolver pass
matched the verified identity after 10.7 seconds and the full CCCD-enable plus
AA command path reached `linked` over LE with no App action or speaker-button
press. A single JBL source was then audible on both speakers. Two requested
cold-control rounds also passed. One intervening scan received 49 BLE events
but no FDDF and failed before command delivery; a later scan saw FDDF again.
The identity mapping and end-to-end transport are therefore verified, while the
FDDF advertising duty cycle remains an operational unknown.

The later deep-standby HCI trace establishes a separate wake order. Android
system BR/EDR A2DP auto reconnect occurred first, followed by stored link-key
authentication/encryption and AVDTP Open. Aura FDDF appeared approximately
`2.5` seconds later, and official-App LE reads occurred later still. A wake
module now implements this ordering in production: stable raw once, on eligible
failure one A2DP ConnectProfile attempt (`20` seconds), fresh exact FDDF
identity/PID within `30` seconds, DisconnectProfile with release confirmed within
`5` seconds, one stable raw retry, then the original LE fallback. One
`150`-second outer deadline bounds the chain; missing release confirmation fails
before role writes. The latest silent no-button cold run completed START in
`122.15` seconds as accepted-unconfirmed/linked through `fresh_le`, then STOP in
`15.89` seconds as accepted-unconfirmed/ready with a clean final journal and
`NRestarts=0`. The overall cold path is hardware-accepted inside `150` seconds;
the A2DP `wake_then_stable` branch was not hit or separately proven. No phone
App participated, but no ADB phone-state evidence was collected.

### Linux transport arbitration

The tested Ubuntu host exposed a repeatable ownership conflict that is separate
from the wire protocol:

- with a phone connected to the Aura, Ubuntu's connection attempt timed out;
- with PulseAudio Bluetooth policy/discovery active, direct PSM 31 attempts
  returned connection abort, refusal, or host-down errors;
- temporarily unloading only those two PulseAudio modules, disconnecting the
  stale local BlueZ session, and waiting one second allowed the same `gatttool`
  transport to write successfully;
- both modules were then reloaded and counted after success and rollback.

Version 0.3 implements this as the `auto` PulseAudio Bluetooth guard while the
persistent sessions are being established. The modules are restored after both
control bearers are ready; the verified sessions stay usable after restoration.
This does not change the command frames or provide evidence about the LE Audio
data plane.

### Control-bearer lifecycle

The bearer lifetime is part of the historical v0.4 working interoperability
procedure, not an implementation detail that can be discarded from that path:

```text
offline
  -> resolve live Aura FDDF identity and connect LE (or use BR/EDR fallback)
  -> connect JBL LE and negotiate MTU 500
  -> ready
  -> JBL ENTER, JBL 7957 action=1, Aura ON
  -> linked
  -> Aura OFF, JBL 7957 action=2, JBL EXIT
  -> ready
  -> close both bearers
  -> offline
```

An earlier one-shot implementation closed both bearers after START. The role
transition succeeded, but the immediately following STOP could not create
fresh connections, so none of its unlink commands reached the speakers. By
contrast, three START and two STOP operations succeeded consecutively through
the same held sessions. The automated supervisor later completed four START and
three STOP operations without a second button press.

The older stable-address-only path had one immediate shutdown/rebuild success
and later `Host is down` failures. Version 0.4 instead resolves the rotating FDDF
identity from fresh typed D-Bus events. Two full no-button cold rounds passed,
and one linked cold start also received a positive two-speaker listening result.
One intervening 30-second FDDF miss shows why the implementation still refuses
to call the path unconditional. Persisting the bearer remains the lowest-latency
daily control solution; a later cold start is an evidenced fallback rather than
a guaranteed fixed-time operation.

The default LE discovery window is 30 seconds. Candidate acceptance requires a
fresh random address plus matching FDDF UUID, PID, and embedded stable identity.
If no such event appears, the supervisor exits before all role writes. It does
not substitute a same-name device, an old BlueZ object, or a persisted RPA.

The safe STOP order is deliberately receiver-first. In the historical v0.4
GATT path, Aura OFF receives its exact Set Device Info success acknowledgement
before JBL `7957 action=2` and EXIT; `7957` had an ATT write ACK. The
JBL-source Rust implementation likewise models Aura OFF before the separate
Assistant broadcaster-stop semantic and JBL EXIT. ENTER/EXIT and Assistant
BasicResponses remain command acceptance rather than complete-ring, acoustic,
`7951`, or BASS/ISO data-plane proof. A failed or uncertain step moves the local
state to `degraded` instead of claiming that the speakers are unlinked.

On hosts with a user systemd manager, the supervisor runs as a transient unit.
Its termination mode signals the Python supervisor before reaping the two
transport children, giving it a bounded opportunity to execute STOP. Graceful
`shutdown` was verified. The raw Aura LE bearer must be closed before asking
BlueZ to restore classic A2DP; the reverse order returned
`br-connection-busy`. Classic A2DP restoration remains best-effort because the
speaker may reject it outside its BR/EDR connectable state. That optional
failure no longer retains the control supervisor. Power failure, speaker
power-off, and unconditional process killing remain non-transactional recovery
cases.

## Separate role namespaces and the DFFD cache caveat

JBL One's generic V3 parser decodes:

```text
byte[14] bits 0..1: 0 NORMAL, 1 BROADCAST, 2 RECEIVER
byte[14] bit 2:     play-state bit
byte[14] bit 3:     disabled
byte[15] bit 0:     feature supported
byte[15] bit 1:     physical-button flag
byte[15] bit 4:     secure mode
```

After the verified two-speaker run, an ordinary `bluetoothctl info` sample from
BlueZ's cache decoded as `2` (RECEIVER). This was not a fresh HCI advertisement
capture, and `2=RECEIVER` comes from JBL One's private enum rather than a
Bluetooth SIG assignment. The repository therefore does not treat it as proof
of either current role or on-air direction.

This cached value does not directly contradict the `7957` request's
`device_status=2`: the latter is a different OneOS enum in which `2` means
broadcaster. Likewise, the Aura's AA general-role value `2` means SECONDARY.
Static controller behavior therefore gives a high-confidence intent of JBL
broadcaster plus Aura secondary, while a fresh `7951` result or an air capture
is still required to establish the resulting live state.

A useful clue from independent PartyBox 520 research is that JBL hardware may
advertise a second LE identity carrying `0x1853` separately from DFFD. The
Bluetooth SIG Assigned Numbers define `0x1853` as **Common Audio Service**, not
Public Broadcast Announcement; the latter is `0x1856`, while Broadcast Audio
Announcement is `0x1852`. The PartyBox observation is therefore evidence of a
separate LE Audio-related identity, but not evidence that it is the public
broadcast source. A fresh capture must look for `0x1852`, `0x1856`, periodic
advertising, BASE, and BIGInfo.

## Standard BASS boundary

The tested Aura exposed handles consistent with Broadcast Audio Scan Service:

| Characteristic | Tested handle |
|---|---:|
| Broadcast Audio Scan Control Point | `0x0023` |
| Broadcast Receive State | `0x0025` |
| Receive State CCCD | `0x0026` |

The original post-success probe returned a generic read error rather than a
value. This is **not equivalent to a standards-defined empty state**:

- [BASS 1.0.1](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/BASS_v1.0.1/out/en/index-en.html)
  says a state with no `Source_ID` shall read as zero length;
- the same specification requires encryption for Broadcast Receive State reads;
- the original probe did not prove that it had established the required
  encrypted GATT security level.

Only a successful encrypted read returning zero bytes would prove an empty
state. The current result is inconclusive, and no claim is made that a generic
Broadcast Assistant could inspect or reproduce the firmware state.

## Firmware portability

Numeric handles are firmware-specific fallbacks. A future implementation should
discover characteristics by UUID and properties. Firmware changes require a
low-volume hardware regression before compatibility is claimed.
