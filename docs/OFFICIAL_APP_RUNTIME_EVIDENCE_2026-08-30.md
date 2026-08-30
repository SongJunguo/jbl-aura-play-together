# Official-App runtime evidence, 2026-08-30

This is a clean-room, sanitized account of one controlled Play Together run on
the supported JBL Authentics 300 and Harman Kardon Aura Studio 5 pair. Raw HCI
snoops, Android bugreports, application logs, UI dumps, device identifiers and
network details remain in the private research workspace and are not included
in this repository.

## Result

The official UI requires **both speakers to be selected into the Play Together
ring**. Adding only the Aura completed the receiver stage but left the JBL tile
selectable and did not establish confirmed two-speaker audio. After the JBL was
also selected, Android A2DP audio was sent to the Aura. Once playback began,
the App/device state identified the Aura as PRIMARY and the JBL as RECEIVER,
and the listener confirmed both speakers. This official home flow therefore
validates the **Aura-source** direction; it does not validate a network source
entering through the JBL.

The controlled run selected the Aura first and the JBL second. The operator has
also repeatedly completed the official flow in the reverse order; the required
postcondition is that both devices are in the ring, not one fixed selection
order. Only the Aura-first order received the full correlated capture in this
run, so order independence is a repeated UI/acoustic observation rather than a
packet-by-packet comparison of both orderings.

This result is evidence for the official applications and device firmware. It
is **not** a pass for the Rust controller. In the clean comparison that
followed, the Home-flow-only Rust build did **not** send `7957`: Aura AA ON and
JBL Wi-Fi ENTER were accepted and local state became `linked`. After the EOF
handling fix, the test waited `15` seconds before Music Assistant sent audio
only to the JBL network player; the listener explicitly confirmed JBL audio
and a silent Aura. This refutes the no-`7957` Home-flow design for the project's
JBL-source target and rules out the earlier `2`-second join delay as the sole
cause. A fixed `10.5`- or `15`-second wait is not a demonstrated repair.

A subsequent exact-GATT candidate restored `7957` on the JBL vendor
characteristic. After the phone Apps and Bluetooth ownership were released,
START was accepted; Music Assistant sent only to the JBL network player at the
requested `5%`, and the user explicitly confirmed that both speakers were
audible. This is the first Rust JBL-source acoustic pass and validates the need
for the GATT `7957` broadcaster step on this pair. That first round's normal
STOP ended `outcome_unknown` with `failure=aura_ack_timeout`; explicit
`recover-stop` returned accepted/`ready` within `13` seconds.

After the fresh-bearer release fix was installed, a second round restarted the
service and again released phone Bluetooth ownership. START was accepted;
Music Assistant targeted only JBL at the requested `5%`, and the user again
confirmed both speakers. Once playback was idle, ordinary STOP completed once
in approximately `43` seconds and returned accepted/`ready` without recovery.
Per the user's two-success agreement, acoustic testing stopped there. The
two-round acoustic gate is complete, but P0/release is not: `7951` remains
unconfirmed, and a prior deep-standby acquisition required the phone's automatic
connection to wake the speaker.

## Exact applications and evidence method

- JBL One `2.7.9` owned the PartyTogether UI and the cross-device operation.
- Harman Kardon One `2.6.11` was used for Aura-side state cross-checks and two
  unsuccessful volume-setting attempts.
- Authorized Android wireless debugging, UI inspection, Bluetooth HCI snoop
  and synchronized network capture were used. No debugging address, local
  forwarding detail, pairing data or phone identifier is retained here.
- Static traces from those exact application versions were compared with the
  live HCI/ATT branch. When static alternatives conflicted with the exact
  runtime branch, the correlated runtime capture was treated as authoritative.

The evidence levels remain separate throughout this report: transport setup,
application-level response, UI state, human acoustic confirmation and on-air
LE Audio data-plane proof are not interchangeable.

## Official UI state machine

The observed path began at the top-right Play Together control on the JBL home
page. It opened `PartyTogether`; it was not the Harman Kardon same-model stereo
pair workflow.

```text
PartyTogether initial page
  -> select Aura
  -> Aura AA ON and correlated success reply
  -> Aura appears in receiver slot
  -> JBL tile remains selectable
  -> select JBL
  -> one JBL 7937 GATT write attempt is rejected as Write Not Permitted
  -> encrypted Wi-Fi record correlates with OneWiFiSession enterAuracast
  -> both devices are in the ring / App returns to JBL home
  -> Android A2DP playback starts on Aura
  -> Aura reports PRIMARY / JBL reports RECEIVER
  -> listener confirms both speakers
  -> remove both devices
  -> Aura AA OFF and correlated success reply
  -> encrypted Wi-Fi record correlates with OneWiFiSession exitAuracast
```

The intermediate UI used a receiver slot for the Aura while the JBL remained
an off/selectable tile. Consequently, `Aura command accepted` is not the same
state as `complete Play Together group`. A controller needs at least distinct
states for receiver-added, both-selected, device-state-confirmed and
acoustically-confirmed.

One screenshot was made while no music was playing. Its silence is not treated
as a negative acoustic sample. The meaningful negative boundary is that the
receiver-only state never received a confirmed two-speaker result; the complete
two-device state did.

## Aura: exact live discovery and command path

The live capture identified the Aura from fresh Harman service data and bound a
rotating LE address to the configured stable identity. Names, RSSI and cached
rotating addresses were not used as identity proof.

| Item | Exact runtime observation |
|---|---|
| Advertisement service-data UUID | `0000fddf-0000-1000-8000-00805f9b34fb` |
| Product ID | `212d` |
| Connect identity | Fresh resolvable private address, bound through the embedded stable identity |
| Vendor service | `65786365-6c70-6f69-6e74-2e636f6d0000` |
| TX write value | UUID suffix `0002`, handle `0x03ea` |
| RX notification value | UUID suffix `0001`, handle `0x03ec` |
| CCCD | UUID `0x2902`, handle `0x03ed` |
| Notification enable | `01 00`, followed by the ATT Write Response |
| Role write | ATT Write Command opcode `0x52` to `0x03ea` |
| AA ON frame | `aa 13 04 00 3c 01 01` |
| AA OFF frame | `aa 13 04 00 3c 01 00` |
| Correlated reply | `aa 00 02 13 00` on `0x03ec` |
| Write-to-reply interval | Approximately `34.2 ms` |

The role write was an ATT Write Command, so it had no ATT Write Response of its
own. The later AA notification is the indispensable application-level success
predicate. The UI placed the Aura in the receiver slot only after this
transaction. During removal, the App sent the corresponding AA OFF frame and
received the same command-family success reply. Both Aura transitions are
direct HCI/ATT observations.

## Static V5 alternative was not the live Aura branch

Both inspected applications contain a HomeBT V5 `V5AuracastMode` feature with
ID `0x2004`. That is genuine static code, but it was not executed in this
transaction. The Aura's `adv_format_4` product route produced a V4-backed
device object, and the live application sent the older AA `0x3c` frame shown
above.

The implementation consequence is precise:

- do not choose V5 merely from the `home_bt` category string;
- route from the exact product and advertisement object;
- keep V5 `0x2004` as a separate alternative, not the default for this pair;
- when an exact-device runtime capture conflicts with a reachable static
  branch, preserve both facts and implement the runtime branch first.

## JBL: Wi-Fi in this run, Bluetooth remains supported

The Authentics 300 has two official control bearers. Static application logic
reuses an online Wi-Fi session when available and otherwise reuses or opens a
GATT session.

The synchronized run exposed both candidate bearers but only one successful
home-flow path:

- selecting the JBL caused one GATT attempt carrying OneOS command `7937`;
  ATT rejected that write as `Write Not Permitted`, so it is a failed attempt,
  not evidence that Bluetooth entered Play Together;
- the successful JBL selection aligned with the UI action, one outbound
  `95`-byte TLS application-data record, the online/Wi-Fi device state, and the
  exact static OneWiFiSession call to `enterAuracast`;
- removing the JBL aligned with one outbound `94`-byte TLS application-data
  record and the matching OneWiFiSession `exitAuracast` call;
- no `7957` operation belonged to this captured PartyTogether home flow.

The TLS records remained encrypted. Their plaintext was not recovered, so the
record sizes are not being presented as decoded commands. The command identity
is a cross-evidence conclusion from timestamp alignment, direction and record
length together with the exact App call graph. It is stronger than an
HCI-only inference but is not a TLS decryption claim.

This does not make the JBL Wi-Fi-only. The App still contains a Bluetooth
fallback, and the run itself shows that it tried one; that particular write was
rejected. The successful transaction for this device state used Wi-Fi.

The trace includes roughly `25` seconds of human operation between UI stages
and roughly `10` seconds before playback was restarted. Those gaps describe
this one manual experiment. They are not protocol delays, retry timers,
settling requirements or implementation constants.

## `7957` transport on the tested JBL firmware

The two available transports did not behave equivalently:

- after certificate-pin matching, HTTPS `setAuracastBroadcast` returned HTTP
  `200` with an `unknown command` device result; HTTP success therefore did not
  mean this firmware accepted `7957` on HTTPS;
- the exact GATT `7957` write on value handle `0x002a` received an ATT write
  acknowledgement. Across the bounded START/STOP exercise, four JBL GATT writes
  received ATT acknowledgements;
- no `7951` business notification was observed. The GATT implementation is
  therefore explicitly ACK-only, even though the later human acoustic check
  proves one successful target-direction START;
- a narrowly scoped callback firewall rule was authorized and installed before
  a production strict-GENA retry. SUBSCRIBE preceded the GATT write, but silent
  START still ended `jbl_broadcast_result_timed_out`; the pair was then
  normalized through the legacy GATT path. The firewall rule is therefore not
  protocol-success evidence, and this exact firmware still has no observed
  `7951` callback.

The acoustic pass raises the evidence level above transport ACK for that one
START. It still does not turn the four ACKs into four business-level successes,
prove standard BASS/BASE/BIG/BIS/ISO behavior, or validate STOP.

Runtime confirmation policy is explicit and closed:

- `JBL_BROADCAST_CONFIRMATION=ack` is the default for this exact firmware. A
  successful ACK-only action returns `accepted_unconfirmed` with evidence
  `broadcast_acknowledgement_only`; the CLI exits `0`, but neither managed
  `linked` nor exit status is a `7951` or acoustic claim;
- `JBL_BROADCAST_CONFIRMATION=gena` is strict. Only matching GENA action `33`
  for START or `34` for STOP returns `accepted` with evidence
  `broadcast_business_notification`; timeout remains a failure.

## Deep-standby wake evidence

The latest HCI trace resolves the phone-assisted wake ordering:

```text
Android system BR/EDR A2DP auto reconnect
  -> stored link-key authentication and encryption
  -> AVDTP Open
  -> approximately 2.5 seconds later, Aura FDDF advertisement
  -> official App LE reads later still
```

This is evidence that the earlier phone involvement was a system A2DP wake
path, not proof that the App's later LE read caused FDDF. A bounded wake module
has now been integrated into production and is present in the latest neutral
artifact. Its cold path performs one stable raw attempt, then on an eligible
failure one A2DP profile connect (`20` seconds), a `30`-second fresh FDDF exact
identity/PID gate, profile disconnect with `5`-second release confirmation, one
stable raw retry, and finally the original LE fallback. All stages share one
`150`-second outer deadline. Missing release confirmation fails before any role
write.

Offline gates are green (`258` library tests, `8` CLI tests, `266` main; FIFO
private-file helper `1/1`; audit, deny, fallback, privacy and neutral), and
compatibility evidence mode is complete.

The latest silent no-button cold run began with no active audio stream, a clean
journal and no resolved ready-made BlueZ device session. START completed in
`122.15` seconds as `accepted_unconfirmed`/`linked`; status reported the exact
two-member pair verified/healthy with route `fresh_le`. STOP completed in
`15.89` seconds as `accepted_unconfirmed`/`ready`; the final journal was clean
and `NRestarts=0`. The phone App did not participate in the transaction, but no
ADB evidence was collected about phone state. The overall no-button cold path
is therefore hardware-accepted within `150` seconds; the A2DP
`wake_then_stable` subpath was not hit or separately proven. No `7951`, acoustic
or BASS/ISO conclusion follows from this silent run.

## Acoustic and application-lifetime checks

In the synchronized official run, the audio bearer was Android A2DP to the
Aura, not a network stream to the JBL. When playback started, Aura became
PRIMARY and JBL was RECEIVER. The applications' control clients could be
released while the established association and phone audio bearer continued;
that does not change the source direction.

- In the earlier Aura-source run, one `5%` setting was inaudible and `10%`
  produced explicit two-speaker confirmation.
- In the later Rust JBL-source GATT-`7957` run, the requested Music Assistant/
  JBL `5%` setting was explicitly audible from both speakers.

The numeric volume alone is therefore not a universal audibility threshold
across source directions and control states.

This demonstrates that the official applications do not need to remain open
to retain the established association. It demonstrates two-speaker audio for
the Aura-source direction only. It does not prove JBL-network-source relay,
standard BASS/BASE/BIG/BIS/ISO behavior, quantify synchronization, or produce a
reliability rate from one captured run.

Two attempts to set the Aura independently to `15%` did not obtain a confirmed
write/readback result. No document or implementation may claim that this
volume was applied.

## Rust JBL-source comparisons

Before the implementation comparison, the operator manually removed both
speakers from the official Play Together ring, stopped playback and force
stopped the applications. This supplied a clean official-flow exit rather than
reusing the successful App state as a Rust precondition.

The tested Home-flow-only Rust build omitted `7957`. After the EOF handling fix,
Aura AA ON and JBL Wi-Fi ENTER were accepted and local state became `linked`.
The test then waited `15` seconds before Music Assistant sent audio only to the
JBL network player. The user explicitly confirmed that the JBL was audible and
the Aura was silent. An earlier clean run of the same direction also ended its
STOP as `outcome_unknown`; that uncertainty still forbids automatic retry or
fallback.

This comparison is directional rather than a byte-for-byte replay:

- the captured official Home flow used Android A2DP into Aura, after which Aura
  was PRIMARY and JBL was RECEIVER, and both were audible;
- the project target sends network audio into JBL. Home-flow-only AA ON plus
  Wi-Fi ENTER reached accepted/local `linked`, but even after `15` seconds only
  JBL was audible.

The result refutes the current no-`7957` design for the JBL-source target. It
also rules out the original `2`-second delay as the sole cause; neither `10.5`
nor `15` seconds may be promoted to a fixed remedy. The active direction now
requires the separate official Assistant `7957` broadcaster semantics for the
JBL together with the Aura receiver-side AA semantics. That is an explicitly
directional composition across two official state machines, not a claim that
one official UI executed the combined sequence.

The exact-GATT candidate then exercised that composition after phone App and
Bluetooth ownership were released. The JBL `7957` write used value handle
`0x002a` and remained ACK-only because no `7951` arrived. START was accepted;
with Music Assistant targeting only the JBL network player at the requested
`5%`, the user explicitly confirmed both speakers. This is the first Rust
target-direction acoustic pass.

The same round did not pass lifecycle acceptance. Ordinary STOP ended
`outcome_unknown` with `failure=aura_ack_timeout`. Explicit `recover-stop`
completed within `13` seconds and returned accepted/`ready`, proving bounded
recovery for this occurrence rather than reliable normal STOP.

The fresh-bearer release fix was then installed. In round two, the service was
restarted and phone Bluetooth ownership was released again. START was accepted;
Music Assistant again targeted only JBL at requested `5%`, and the user again
confirmed both speakers. After playback became idle, ordinary STOP completed in
approximately `43` seconds and returned accepted/`ready` with no recovery. The
user had requested that sound testing stop after two successful rounds, so no
third acoustic round was attempted.

This closes the requested two-round acoustic gate and supplies one post-fix
normal STOP pass. It does not complete P0 or release acceptance: no `7951` was
observed, only one post-fix normal STOP is recorded, and a prior deep-standby
case required the phone's automatic connection to wake the speaker before
control could proceed.

## Invalid earlier trials and why they failed as evidence

Two earlier audio attempts were excluded before interpreting the official run:

- one collided with an automatic Home Centre STOP in the same experimental
  window;
- another lost the directly launched control daemon while playback was active.

Legacy-service lock conflicts and failed restarts were also present in that
earlier period. Those attempts had no single writer and no complete ownership
record, so they are neither protocol successes nor protocol failures. They
explain why transport acknowledgements and retained membership previously
produced false confidence about an audible group.

The recurring Aura-silent outcome had four concrete causes in the experimental
method and implementation assumptions:

1. treating an Aura acknowledgement as complete-group success;
2. treating retained two-member topology as a live linked state;
3. selecting the static V5 path instead of the observed V4/AA route;
4. assuming the no-`7957` Home flow validated the project's opposite source
   direction. The simultaneous capture shows that Home flow used Aura as
   PRIMARY and JBL as RECEIVER.

## End state and implementation boundary

The operator manually removed both speakers from the official ring and stopped
playback before the Rust comparison. After the later exact-GATT START pass,
ordinary STOP became outcome-unknown because the Aura acknowledgement timed
out. The explicit single-writer recovery then completed within `13` seconds and
returned accepted/`ready`. This records a recovered incident, not a passing
normal STOP path. After the fresh-bearer release fix, round two ended with an
ordinary approximately `43`-second STOP accepted/`ready` and no recovery. The
latest managed end state for this evidence sequence is therefore `ready`.

Historical v0.4 two-speaker audio remains compatibility/regression evidence.
The Home-flow-only Rust transaction deliberately omitted `7957` and used Aura
AA ON plus JBL Wi-Fi ENTER. After the EOF fix, those commands were accepted and
local state became `linked`, yet a JBL-only network source still produced no
Aura audio after a `15`-second wait. That target-direction design is refuted.
The exact-GATT candidate reintroduced the separate JBL Assistant `7957`
broadcaster semantics as a cross-state-machine composition and obtained the
requested two Rust target-direction acoustic passes. It is not described as one
official UI transaction. The fresh-bearer release fix also produced one normal
STOP accepted/`ready` result, but P0 remains incomplete because `7951`, deeper
the unhit A2DP `wake_then_stable` subpath and the remaining release gates are
unresolved.

## Evidence matrix

| Claim | Evidence | Status |
|---|---|---|
| Official Aura operation used fresh FDDF identity binding | Correlated advertisement, LE connection and device identity | Observed |
| Exact Aura runtime command is AA/V4 `0x3c` | HCI ATT write and correlated AA notification | Observed |
| Aura used static V5 `0x2004` in this run | No V5 frame; exact AA frame observed | Refuted |
| Aura reply alone completes Play Together | UI still showed JBL as selectable | Refuted |
| Both speakers must be in the official ring | UI plus completed acoustic check | Observed |
| Selection order is fixed | Operator completed both orders in official App | Refuted at UI/acoustic level; only one order packet-correlated |
| Captured Home flow validates JBL as audio source | Android A2DP entered Aura; Aura became PRIMARY and JBL RECEIVER | Refuted; Home flow validates Aura-source direction |
| Successful JBL home-flow control used Wi-Fi | UI timestamp, encrypted `95`/`94`-byte TLS records and exact OneWiFiSession enter/exit call graph | Correlated cross-evidence |
| JBL is Wi-Fi-only | Official Bluetooth fallback and other observed App use | Refuted |
| JBL `7937` GATT attempt completed ENTER | ATT returned Write Not Permitted | Refuted for this attempt |
| Official home flow used `7957` | Aura AA plus Wi-Fi enter/exit sequence contained no `7957` step | Refuted for this captured flow |
| Exact JBL network plaintext was captured | Synchronized capture retained encrypted TLS records only | Not observed; no decryption claim |
| Official removal completed both sides | Aura AA OFF succeeded and the encrypted Wi-Fi record correlated with `exitAuracast` | Observed/correlated |
| Control Apps must remain open for playback | Association continued while Android A2DP still supplied Aura | Refuted for this Aura-source run |
| `5%` is a universal audibility threshold | Earlier Aura-source run was inaudible at one `5%` setting; later Rust JBL-source run was dual-audible at requested `5%` | Refuted |
| Observed human timing is a protocol delay | Approximately `25`- and `10`-second gaps came from this manual run | Refuted as a requirement |
| Aura was successfully set to `15%` | No confirmed response/readback | Not verified |
| EOF-fixed Home-flow-only Rust produces JBL-source two-speaker audio | START accepted/local linked; after `15` seconds, JBL audible and Aura silent | Refuted on this pair |
| A fixed `10.5`- or `15`-second delay repairs the JBL-source path | The `15`-second negative excludes the original `2`-second delay as the sole cause | Refuted as a demonstrated fix |
| The clean Home-flow-only Rust comparison sent `7957` | That tested backend omitted the separate Assistant command | Refuted |
| Official Home-flow success proves current JBL-source Rust success | Official source was Aura; project source was JBL | Refuted by direction mismatch |
| Reintroduced `7957` is the same official UI sequence | It comes from the separate Assistant broadcaster state machine | Refuted; explicit cross-state-machine composition |
| HTTPS `7957` works when TLS pinning succeeds | HTTP `200` carried an `unknown command` device result | Refuted on this firmware |
| Exact GATT `7957` target-direction START produces two-speaker audio | Two STARTs accepted; Music Assistant targeted only JBL at requested `5%`; user confirmed both twice | Verified for two requested rounds |
| Four JBL GATT ACKs prove four business successes | No `7951` was observed | Refuted; ATT layer only |
| Strict GENA produced `7951` after the narrow firewall rule | Silent START timed out as `jbl_broadcast_result_timed_out` | Refuted on this firmware |
| The installed firewall rule proves protocol success | Strict production retry still timed out | Refuted |
| ACK mode is business-confirmed | Returns `accepted_unconfirmed` plus `broadcast_acknowledgement_only`, CLI exit `0` | Refuted by explicit semantics |
| Managed `linked` proves `7951` or audibility | It records the last accepted controller action | Refuted |
| First-round normal STOP passed | `outcome_unknown`, `failure=aura_ack_timeout` | Refuted; explicit recovery succeeded within `13` seconds |
| Post-fix round-two normal STOP returns ready | Playback idle; ordinary STOP accepted/`ready` in approximately `43` seconds | Verified once, without recovery |
| More sound rounds are required for the agreed acoustic gate | User requested stopping after two successful rounds | Refuted; acoustic testing stopped |
| Production no-button cold path completes in hardware | START `122.15` s via `fresh_le`; STOP `15.89` s; clean journal; `NRestarts=0` | Verified for one silent round |
| Phone wake was caused by the later App LE read | BR/EDR A2DP reconnect, link-key auth/encryption and AVDTP Open preceded FDDF by about `2.5` seconds | Refuted |
| Wake module is present in production artifact | Bounded cold chain and shared `150`-second deadline pass offline | Verified offline |
| Wake profile release can be assumed | Missing release confirmation fails before role writes | Refuted by design |
| A2DP `wake_then_stable` passed hardware acceptance | Latest run used `fresh_le`; subpath was not hit | Not tested separately |
| Phone state was verified absent | No App participation, but no ADB state evidence | Not claimed |
| Offline release gates pass | `258` library + `8` CLI (`266` main), FIFO private-file helper `1/1`, audit/deny/fallback/privacy/neutral | Verified |
| Compatibility evidence mode is complete | Dedicated evidence gate passed | Verified offline |
| Play Together P0 is complete | Acoustic rounds and silent no-button cold pass exist; `7951`, `wake_then_stable` and release gates remain | Not complete |
| Standard Auracast data plane was proven | No packet-level BASS/BASE/BIG/BIS/ISO proof | Not proven |

## Clean-room publication boundary

This document intentionally contains no real Bluetooth address, private IP,
hostname, username, account data, media URI, certificate, key, phone identifier,
raw log excerpt, capture filename or absolute private path. Only minimal
interoperability facts needed to build synthetic fixtures are published.

Contributor work follows the repository's
[official-App-first and dynamic-evidence rules](../AGENTS.md). Raw application
and phone evidence must remain private; public tests must use synthetic data.
