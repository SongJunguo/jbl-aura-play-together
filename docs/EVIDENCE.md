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

Static analysis of the two current controller apps independently corroborated
the control-plane meaning after the hardware run: JBL One `2.7.9` builds
`7957 action=1` as a broadcast request for the controlled JBL and waits for
`7951`; Harman Kardon One `2.6.11` constructs the exact seven-byte AA ON frame
and recognizes the captured five-byte reply as Set Device Info success. No
decompiled source or app binary is included here.

The same pair was retested later that day with an explicit baseline. While a
phone held the Aura connection, one Ubuntu attempt timed out after 20 seconds.
After the phone was disconnected, the host used the combined procedure of
isolating the two PulseAudio Bluetooth modules, releasing the stale BlueZ
session, and waiting briefly. The vendor sequence then completed, both modules
were restored, and a listener again confirmed that both speakers were audible
at low volume. This trial does not isolate which guard step was individually
necessary or establish long-term reliability.

No household address, network address, device UUID, log, capture, or account
identifier is included in this public record.

## Official-controller dynamic cross-check

On 2026-08-29, JBL One `2.7.9` was operated through its top-right Play Together
icon. Its Play Together (`AuraCastActivity`) page discovered the Aura Studio 5
as an available speaker. Selecting it moved the Aura into the UI's first
receiver slot. After leaving and reopening the page, the receiver slot remained
populated.

No audio was started during this pass, and release logcat did not expose a
fresh, sanitizable `7951` result. This confirms the current official controller
offers and retains the exact cross-model role transition, but it does not add a
new listening result or prove the on-air data plane. The Harman Kardon One
"create a stereo pair" page was separately checked and exited: it requests a
second unit of the same model and is not the PartyTogether mechanism used here.

The one-shot v0.2 lifecycle was then tested more strictly. Its `start` could
complete, but an immediate `stop` could no longer create fresh control
connections after the role transition. The stop commands were therefore not
delivered. This is a counterexample to treating independent one-shot start and
stop transactions as reliable, even though earlier isolated transactions had
worked.

## Persistent-session lifecycle evidence

The corrected architecture connects both control bearers before changing any
role and keeps them open. In an interactive real-device proof, the same two
held sessions completed three START operations and two STOP operations without
another App action, button press, re-pair, or reconnect. Every Aura ON/OFF
returned `aa00021300`; every JBL ENTER/EXIT returned a BasicResponse with
`error_code=0`; every JBL `7957` start/stop write had an ATT write ACK. A fresh
`7951` notification was still not observed.

The v0.3 automated supervisor was then tested on the same host. One Aura button
press cleared the exceptional state left by previous unmanaged experiments.
After that single recovery action, the supervisor completed four managed START
operations and three managed STOP operations without another button press.
`status` alternated between `linked` and `ready`, the transient user-systemd
service remained alive across separate CLI processes, and the PulseAudio
Bluetooth modules were restored while the control sessions remained usable.

One immediate `shutdown` followed by a fresh `start` rebuilt both sessions and
linked without another press. A later repetition after the Aura's classic
connectable window had closed failed at the initial Aura connection with
`Host is down`, before any role command was sent. That early scan used the
ordinary BlueZ surface and incorrectly closed the investigation: later HCI and
typed D-Bus work found the rotating FDDF control identity described below.

No music was played during this automated lifecycle pass. It proves repeatable
delivery and the held-session fix on this host, not a new audible result, BASS
subscription, or ISO data path.

The final supervisor added a bounded Aura acquisition loop rather than a single
connection attempt. An injected fixture failed the first two Aura connections
and then passed; the complete offline state-machine, failure, privacy, and
wrapper suites passed. On hardware, arming the loop before the short physical
connectability interval established the final persistent session. The resulting
user-systemd unit reported `linked`, `KillMode=mixed`, both PulseAudio Bluetooth
modules present, approximately 11 MiB resident memory, and a passing `doctor`.
The session was intentionally left running; no final shutdown was performed.

## Rotating-LE discovery evidence

On 2026-08-29, a phone HCI capture showed a connectable random Aura identity
with Harman `0xFDDF` Service Data, PID `212d`, the stable BR/EDR address at
payload bytes 11-16, the Excelpoint control service, and the same control
handles as the working BR/EDR database. Independent PartyBox 520 research
documents the same FDDF identity offset and advises connecting from the live
scan result rather than persisting an RPA.

The experimental resolver was rewritten to use BlueZ D-Bus directly. It calls
`SetDiscoveryFilter` with LE transport, duplicate reporting, and an empty
pattern; consumes typed ObjectManager and Properties signals; and accepts a
candidate only when FDDF, PID, and embedded stable identity all match. A
read-only hardware probe resolved the Aura from five fresh advertisement
events, with one FDDF payload and one identity match. No address was logged.

A first forced-LE START trial then failed safely before any role write. Android
state showed that the phone had automatically reconnected to Aura A2DP after
the read-only probe. While that competing link was present, two typed scans and
one interactive BlueZ scan observed other advertisements but no Aura FDDF; the
supervisor reported failure and restored both PulseAudio modules. This proves
the resolver is not willing to substitute a stale cache entry.

The phone was then disconnected through Android's system Bluetooth UI. Android
state showed the Aura A2DP profile disconnected and no active Harman GATT client.
Without an App action or speaker-button press, a 60-second read-only resolver
request matched the Aura after 10.7 seconds (`21` advertisement events, one
FDDF payload, one identity match). The following managed cold start reached
`linked` with `aura_transport=le`. A source was played only through the JBL at
15% volume, and the listener selected the explicit result "both speakers are
audible." Playback was then stopped.

Per the requested bounded acceptance gate, two complete no-button cold-control
rounds were obtained and testing stopped. The successful rounds took 17 and 15
seconds and each ended `ready` after a verified STOP. They were not consecutive:
one intervening 30-second attempt saw 49 advertisement events but zero FDDF,
sent no role command, restored both PulseAudio modules, and exited failed. A
later passive scan matched one FDDF identity after 10.7 seconds (`22` events),
and the second cold round then passed. This is evidence for unattended cold
reconnect capability with an advertising-window caveat, not a claim of
unconditional first-attempt reliability.

The same pass exposed a separate shutdown bug. Restoring classic A2DP while the
raw Aura LE control bearer was still held returned `br-connection-busy`.
Releasing LE first removed that ordering error, although the speaker could still
reject the optional classic profile with `Host is down`. Version 0.4 therefore
closes the control supervisor first and treats bounded A2DP restoration as a
separate best-effort handoff. A rejected A2DP handoff is reported and its private
marker retained; it no longer keeps the control supervisor alive.

## Evidence matrix

| Claim | Evidence | Status |
|---|---|---|
| JBL accepted the bytes | GATT write ACK | Observed at ATT layer |
| JBL app intends `7957` to enable this device's broadcaster role | Current app request construction | Decompiled control-plane fact |
| Aura accepted ON | AA response for Set Device Info | Observed |
| Aura AA token and ACK semantics | Current app command builder and response predicate | Decompiled control-plane fact |
| Official JBL controller places Aura in receiver slot | Live UI transition persisted after activity reopen | Observed control-plane state |
| Aura entered secondary/on state | AA device-info tokens | Observed |
| It was not Linux dual-sink output | Aura A2DP absent; bridge stopped | Verified |
| Both speakers were audible | Human listening confirmation | Verified |
| Standard BASS source was added | Read failed; encrypted access was not proven | Inconclusive |
| JBL was on-air standard broadcast source | App intent says broadcaster; DFFD is a different enum/cache; no fresh `7951` or ISO capture | Not proven |
| Exact synchronization error | No dual-microphone measurement | Not measured |
| Phone and Ubuntu can control Aura concurrently | Ubuntu timed out while phone held it | Refuted on tested state |
| Tested PulseAudio modules were restored | Both named modules reloaded after success and failed-start rollback | Verified on this host |
| Independent one-shot start then stop is reliable | Immediate stop could not reconnect after role change | Refuted on tested state |
| Held-session stop avoids post-role reconnect | Manual 3 START / 2 STOP series with exact control acknowledgements | Verified on this host |
| v0.3 supervisor survives separate CLI invocations | Automated 4 START / 3 STOP held-session series | Verified on this host |
| Bounded acquisition survives transient Aura misses | Two injected misses plus final armed hardware acquisition | Verified |
| Typed D-Bus scan resolves current Aura RPA | Fresh FDDF/PID/stable-address match | Verified read-only |
| Resolver accepts cached/stale RPA when FDDF is absent | Competing-phone trial returned no candidate and no writes | Refuted |
| Forced-LE cold START with no competing host | Phone-disconnected, no-button trial reached `linked` over LE | Verified on this pair |
| No-button cold reconnect can be repeated | Two successful cold rounds, with one safe FDDF-window miss between them | Verified with an advertising-window caveat |
| Every immediate post-shutdown start succeeds | 30-second scan saw 49 events but no FDDF and correctly sent no command | Refuted on tested state |
| Automated cold lifecycle produced two-speaker audio | Single JBL source at 15%; listener explicitly confirmed both speakers | Verified |
| Shutdown can restore classic A2DP before closing LE | BlueZ returned `br-connection-busy`; release-first ordering is required | Refuted on tested host |
| Delayed FDDF retry covers an observed advertising gap | First 30-second service scan missed; 15-second delay plus rescan reached `ready/le` | Verified on this host |
| Installed service recovers an idle Aura bearer loss | Deliberate Aura control-child termination caused nonzero exit, systemd restart, role normalization and `ready/le` | Verified on this host |
| Boot-service control stays separate from Aura A2DP | Deployed service forced to LE; both PulseAudio modules present and no Aura sink after recovery | Verified on this host |

## Failed or incomplete approaches

- Independent AirPlay and A2DP output produced audible timing offset.
- `enterAuracast` alone did not make the second speaker audible.
- The separate OneOS named-group `SET_AURA_CAST_GROUP` route produced an
  unsuitable role state for this cross-generation pair.
- Aura AA ON without an available coordinated source did not yield a readable
  BASS state in the original low-security probe.
- Keeping the Aura attached to the Linux A2DP sink conflicted with the vendor
  association path.
- Closing both control bearers after `start` made a later one-shot `stop`
  dependent on fresh connections that the role-changed speakers could reject.
- BlueZ 5.64 lacks the newer `bluetoothctl assistant` interface.
- A post-link ordinary BlueZ scan cached DFFD but no Common Audio Service
  `0x1853`, Public Broadcast Announcement `0x1856`, or Harman manufacturer-87
  identity. The scan did not test Broadcast Audio Announcement `0x1852`, and a
  negative BlueZ cache result is not proof that no periodic advertisement or
  BIG/BIS existed.

## Open questions

1. Does a second JBL LE identity advertise `0x1852`/`0x1856` and originate the
   periodic advertisement and BIG/BIS?
2. Was the post-link DFFD sample fresh, and how does that advertisement role
   evolve relative to the separate OneOS broadcast-status state machine?
3. Does the Aura intentionally hide vendor-added receive state from its standard
   BASS characteristic?
4. Is `7951` emitted on another transport or only in specific firmware states?
5. How much acoustic offset remains when measured with two microphones?
6. What controls the Aura FDDF advertising duty cycle or post-disconnect
   cooldown, and what retry/backoff policy gives the best first-command success
   rate without accepting a stale RPA?

Pull requests with sanitized, reproducible evidence are welcome. Do not attach
raw HCI logs or device identifiers to public issues.
