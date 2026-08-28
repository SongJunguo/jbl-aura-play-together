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
OneOS devices and waits for all of them to enter group mode. The tested
Authentics-plus-Aura sequence uses `7957` on the JBL and the separate AA command
below on the Aura.

Dynamic controller inspection adds an important provenance boundary. JBL One's
visible PartyTogether activity uses the ENTER/EXIT state machine and persists
the selected Aura in a receiver slot. The `7957` request builder belongs to a
separate broadcaster-assistant flow. The Linux sequence combines these two
controller-supported semantics; it is not claimed to be a packet-for-packet
recording of one official PartyTogether UI transaction.

**Important:** the tested JBL acknowledged the ATT write, but no `7951`
application-level success notification was captured. A write ACK is necessary,
not sufficient, evidence.

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
only as a fallback. ATT PSM 31 lets Linux use the stable classic identity and
avoid hard-coding a rotating LE address.

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

The bearer lifetime is part of the working interoperability procedure, not an
implementation detail that can be discarded:

```text
offline
  -> connect Aura PSM 31
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

One immediate shutdown/rebuild happened to reconnect, but a later fresh start
after the Aura's connectable window closed failed with `Host is down` before
command delivery. No separate named or PID-matching V4/FDDf LE advertisement
was visible in that state. Persisting the bearer is therefore the verified
solution; cold reconnect without a physical Bluetooth-button press is not.

To avoid missing the short physical window through process-launch latency,
v0.3 repeatedly issues the Aura connect request every 250 ms within a bounded
45-second acquisition period. The supervisor is armed first; a button press at
any point in that period can be captured. This improves acquisition timing but
does not remotely open a radio window that the speaker has already closed.

The safe STOP order is deliberately receiver-first: Aura OFF must receive its
exact Set Device Info success acknowledgement before JBL `action=2` and EXIT
complete. ENTER and EXIT require the JBL BasicResponse `error_code=0`; `7957`
currently has only an ATT write acknowledgement because no fresh `7951` was
captured. A failed step moves the local state to `degraded` instead of claiming
that the speakers are unlinked.

On hosts with a user systemd manager, the supervisor runs as a transient unit.
Its termination mode signals the Python supervisor before reaping the two
transport children, giving it a bounded opportunity to execute STOP. Graceful
`shutdown` was verified. Power failure, speaker power-off, and unconditional
process killing remain non-transactional recovery cases.

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
