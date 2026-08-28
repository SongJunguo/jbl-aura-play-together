# Protocol notes

These notes describe interoperability observed on one tested firmware pair.
They are not an official JBL or Harman Kardon specification.

Evidence labels used below:

- **Observed**: captured from the local device or transport.
- **Decompiled**: reconstructed from data structures in JBL One 2.7.9.
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
| 7942 | `0x1f06` | legacy `SET_AURA_CAST_GROUP` |
| 7943 | `0x1f07` | `DESTROY_AURA_CAST_GROUP` |
| 7950 | `0x1f0e` | `GET_DEVICE_AURACAST_BROADCAST_INFO` |
| 7951 | `0x1f0f` | broadcast-info notification |
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

This is Set Device Info (`0x13`) with token `0x3c`. The successful hardware run
observed response `aa 00 02 13 00`, then device-info values corresponding to
SECONDARY and Play Together ON.

The official app prefers its BR/EDR control session and uses a random LE address
only as a fallback. ATT PSM 31 lets Linux use the stable classic identity and
avoid hard-coding a rotating LE address.

## DFFD advertisement role discrepancy

JBL One's generic V3 parser decodes:

```text
byte[14] bits 0..1: 0 NORMAL, 1 BROADCAST, 2 RECEIVER
byte[14] bit 2:     play-state bit
byte[14] bit 3:     disabled
byte[15] bit 0:     feature supported
byte[15] bit 1:     physical-button flag
byte[15] bit 4:     secure mode
```

In the verified two-speaker run, the role bits were still `2` (RECEIVER). The
repository deliberately does not reinterpret that as BROADCAST.

A useful clue from independent PartyBox 520 research is that JBL hardware may
advertise a second LE identity carrying the Bluetooth Public Broadcast
Announcement service (`0x1853`) separately from DFFD. That could mean DFFD is
not authoritative for the actual LE Audio advertising set. This is an
**inference**, not a resolution of the Authentics/Aura observation.

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
