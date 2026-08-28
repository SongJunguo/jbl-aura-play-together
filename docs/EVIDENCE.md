# Evidence and unresolved questions

## Verified hardware result

On 2026-08-28, with a JBL Authentics 300 on firmware `26.24.31.50.00` and a
Harman Kardon Aura Studio 5:

1. A single audio source was playing on the JBL.
2. The Aura A2DP sink on Linux was absent and the previous independent Bluetooth
   bridge was stopped, excluding Linux-side dual-sink playback.
3. The JBL transport accepted the PL `7957 action=1` write after MTU 500
   negotiation.
4. The Aura transport accepted `aa1304003c0101` and returned
   `aa00021300` in a notification-enabled diagnostic session.
5. Aura device information reported SECONDARY and Play Together ON.
6. A listener explicitly confirmed that both speakers produced the test audio.

No household address, network address, device UUID, log, capture, or account
identifier is included in this public record.

## Evidence matrix

| Claim | Evidence | Status |
|---|---|---|
| JBL accepted the bytes | GATT write ACK | Observed at ATT layer |
| Aura accepted ON | AA response for Set Device Info | Observed |
| Aura entered secondary/on state | AA device-info tokens | Observed |
| It was not Linux dual-sink output | Aura A2DP absent; bridge stopped | Verified |
| Both speakers were audible | Human listening confirmation | Verified |
| Standard BASS source was added | Read failed; encrypted access was not proven | Inconclusive |
| JBL was standard broadcast source | DFFD said RECEIVER; no ISO capture | Not proven |
| Exact synchronization error | No dual-microphone measurement | Not measured |

## Failed or incomplete approaches

- Independent AirPlay and A2DP output produced audible timing offset.
- `enterAuracast` alone did not make the second speaker audible.
- The older `SET_AURA_CAST_GROUP` route produced an unsuitable role state.
- Aura AA ON without an available coordinated source did not yield a readable
  BASS state in the original low-security probe.
- Keeping the Aura attached to the Linux A2DP sink conflicted with the vendor
  association path.
- BlueZ 5.64 lacks the newer `bluetoothctl assistant` interface.

## Open questions

1. Is a second JBL LE advertising identity the actual Public Broadcast
   Announcement/Broadcast Audio source?
2. Why does the Authentics DFFD role remain RECEIVER after audible success?
3. Does the Aura intentionally hide vendor-added receive state from its standard
   BASS characteristic?
4. Is `7951` emitted on another transport or only in specific firmware states?
5. How much acoustic offset remains when measured with two microphones?
6. Does association survive full power loss and repeated start/stop cycles?

Pull requests with sanitized, reproducible evidence are welcome. Do not attach
raw HCI logs or device identifiers to public issues.
