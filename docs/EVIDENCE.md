# Evidence and unresolved questions

The Rust LAN/model/group and native-control milestones are recorded separately
in [`RUST_LAN_EVIDENCE_2026-08-30.md`](RUST_LAN_EVIDENCE_2026-08-30.md). The
initial exact-member check was read-only; later controlled Rust lifecycle and
recovery trials did change speaker control state. They complement rather than
rewrite the dated v0.4 evidence below.
The sanitized read-only selection of mTLS HTTPS as the next JBL control channel
is recorded in
[`JBL_CONTROL_CHANNEL_2026-08-30.md`](JBL_CONTROL_CHANNEL_2026-08-30.md).
The later controlled START/STOP comparison showed that the two-member API is a
retained membership configuration rather than a live linked-state signal; see
[`GROUP_MEMBERSHIP_SEMANTICS_2026-08-30.md`](GROUP_MEMBERSHIP_SEMANTICS_2026-08-30.md).
The exact official-App runtime cross-check that resolved the Aura V4/AA branch,
the two-device ring requirement, the JBL bearer used in that run and the
post-App acoustic result is recorded in
[`OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md`](OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md).

## Rust native-control checkpoint

On 2026-08-30, the Rust controller first repeated the real-device read with
exact private member-ID matching and reported the expected pair configuration
ready. No identity value was logged. The already-recorded STOP comparison still
applies: this retained configuration is an identity/topology prerequisite, not
a live linked or audible signal.

The native whole-pair path then produced accepted Rust `start` and `stop`
results. Two no-button cold `start` rounds succeeded; managed status reported
`br_edr` during the first and ended `le` during the second. Two normal STOP
actions through the persistent session completed in approximately 0.44 and
0.57 seconds.

The negative and recovery cases were also exercised:

- one real STOP encountered an FDDF advertising-window miss, rejected before
  any device write, and left the write-ahead journal clean;
- a direct connection to the discovered LE `Device1` failed. A bounded nudge
  through the paired-and-trusted stable public object caused BlueZ to connect;
  the vendor GATT service was actually exposed on the unique connected random
  object, which Rust adopted only after exact FDDF PID and embedded-stable-
  identity matching;
- an explicit recovery performed safe diagnosis, proved the paired stable
  identity, mapped it to the current FDDF random identity, received the required
  acknowledgements and returned managed state to `ready`;
- an older timeout construction caused a real panic only after the pending
  journal record was committed. Reopen preserved the unresolved action and
  blocked ordinary writes. The timeout path was fixed, while the pending state
  remained protected until the explicit recovery succeeded.

Two release-blocking crash/teardown gaps were subsequently closed. Graceful
shutdown now propagates teardown failure as a nonzero process exit, while a
teardown latch prevents later cleanup from masking the uncertain outcome. An
independent owner-only `uncertainty.pending` marker is authoritative across
restart, so even a failed directory sync while recording clean state leaves
ordinary mutations blocked until explicit recovery. These are offline
crash-consistency guarantees, not new device-state evidence.

The Web listener exit boundary was also closed: accept/serve errors return the
controller actor, and both normal and abnormal listener exits execute exactly
one `shutdown_for_exit`. Device-safety errors retain priority; if shutdown
succeeds, the original `AcceptFailed` remains the reported error. This path does
not clear a pending journal.

The approximately 03:45 full-song Rust attempt is not an acoustic gate. Home
Centre issued an automatic STOP in the same experimental window, so the
reported JBL-only audio was contaminated by a concurrent writer and supports no
protocol conclusion. A later clean test used the official-derived build whose
Home-flow-only backend did not send `7957`. After the EOF handling fix, Aura AA
ON and JBL Wi-Fi ENTER were accepted and managed state became `linked`. The
test waited `15` seconds before Music Assistant sent audio only to the JBL
network player; the user explicitly confirmed JBL audio and a silent Aura.
This refutes the no-`7957` design for the project's JBL-source direction and
excludes the original `2`-second delay as the sole cause. Fixed `10.5`- or
`15`-second waits are not demonstrated repairs. An earlier clean STOP also
ended `outcome_unknown` and left a pending record. Standard
BASS/BASE/BIG/BIS/ISO data-plane evidence remains absent.

The Rust daily service and UI use loopback port `8096`; Music Assistant remains
separate on `8095`. The v0.4 backend remains available only as an explicit
fallback. There is no automatic backend failover or write replay after a
rejected or uncertain action.

An earlier user-service deployment checkpoint exposed two Ubuntu 22.04 compatibility
constraints before the executable could run: user units could not apply the
capability-changing hardening directives, and `RestrictSUIDSGID` interfered
with the owner-only `openat2` configuration read. Those incompatible settings
were removed while `NoNewPrivileges`, read-only home/system protection and the
remaining namespace/memory restrictions stayed enabled. The address-family
allowlist explicitly includes `AF_BLUETOOTH`; omitting it made the same ELF's
device connection fail inside systemd while succeeding in a normal user
process. With that correction the installed Rust unit reached `ready/healthy`,
the private journal was clean, Home Centre was restored with desired state
`ready`, and the v0.4 unit remained inactive. Neither service had restarted in
the post-install observation window.

## Simultaneous official-App control capture

A later synchronized UI, Bluetooth HCI and network capture resolved the
official PartyTogether home flow without exposing private identifiers. Aura ON
used the already-recorded AA ATT Write Command and received its correlated
success notification. One JBL `7937` GATT write was attempted, but ATT returned
`Write Not Permitted`; that attempt did not enter the JBL into the group.

The successful JBL selection aligned with the UI action, one outbound
`95`-byte encrypted TLS record, the online/Wi-Fi device state and the exact
OneWiFiSession `enterAuracast` call graph. Removal produced a successful Aura
AA OFF and one outbound `94`-byte encrypted TLS record aligned with
OneWiFiSession `exitAuracast`. No `7957` step belonged to this captured home
flow. The audio source in this run was Android A2DP into Aura; after playback
began, Aura was PRIMARY and JBL was RECEIVER, and the listener confirmed both
speakers. The Home flow therefore validates the Aura-source direction, not a
network source entering through JBL.

The TLS plaintext was not decrypted. Command identity is therefore a
cross-evidence conclusion from UI time, record direction and length, and the
exact static call path—not a claim that ciphertext was decoded. Approximately
`25` seconds between manual UI stages and approximately `10` seconds before
playback resumed are operator timing from this run, not protocol delays or
required settling periods.

The project target has the opposite direction: network audio enters through
JBL and Aura must receive it. That candidate therefore reintroduces the
separate official Assistant `7957` JBL-broadcaster semantics alongside the Aura
AA receiver semantics. This is an explicit directional composition across two
official state machines, not a claim that one official UI emitted the combined
transaction.

## First Rust JBL-source acoustic pass

The exact-GATT candidate was then tested after the phone Apps and Bluetooth
ownership were released. Its `7957` write used JBL value handle `0x002a` and
remained ACK-only because no `7951` notification arrived. START was accepted;
Music Assistant targeted only the JBL network player at the requested `5%`, and
the user explicitly confirmed that both JBL and Aura were audible. This is the
first Rust target-direction START/data-plane pass and validates the need for the
GATT `7957` broadcaster step on this pair.

The transports were not interchangeable. After certificate-pin matching,
HTTPS `setAuracastBroadcast` returned HTTP `200` with an `unknown command`
device result. Across the bounded GATT START/STOP exercise, four JBL GATT writes
received ATT acknowledgements, but those ACKs are not substitutes for `7951` or
four business-level successes. No GENA callback was observed; the later narrow-
firewall strict retry also timed out, so installing that rule is not a fix.

Lifecycle acceptance did not pass. Ordinary STOP ended `outcome_unknown` with
`failure=aura_ack_timeout`. Explicit `recover-stop` completed within `13`
seconds and returned accepted/`ready`.

After the fresh-bearer release fix was installed, round two restarted the
service and again released phone Bluetooth ownership. START was accepted;
Music Assistant targeted only JBL at requested `5%`, and the user again
confirmed both speakers. With playback idle, ordinary STOP completed once in
approximately `43` seconds and returned accepted/`ready` without recovery. Per
the user's two-success agreement, sound testing stopped there.

The requested acoustic gate is now two-for-two and one post-fix normal STOP has
passed. P0/release remains incomplete: `7951` is still unconfirmed, a prior
deep-standby case needed the phone's automatic connection to wake the speaker,
and the remaining non-acoustic release gates are not frozen.

Strict GENA was then retried in production after the user authorized and
installed a narrowly scoped callback firewall rule. SUBSCRIBE preceded the
exact GATT write, but silent START still ended
`jbl_broadcast_result_timed_out`; the pair was normalized through the legacy
GATT path. The rule is not protocol-success evidence, and this exact firmware
still has no observed `7951`.

Confirmation semantics are intentionally explicit. The closed setting
`JBL_BROADCAST_CONFIRMATION=ack|gena` defaults to `ack` for this pair. ACK mode
returns `accepted_unconfirmed` with `broadcast_acknowledgement_only` and CLI
exit `0`. Strict `gena` returns `accepted` with
`broadcast_business_notification` only for matching action `33`/`34`; timeout
is failure. Managed `linked` records an accepted controller action and is not a
`7951` or acoustic claim.

The latest HCI trace also resolves the prior phone-assisted deep-standby wake:
Android system BR/EDR A2DP auto reconnect occurred first, followed by stored
link-key authentication/encryption and AVDTP Open. Aura FDDF appeared about
`2.5` seconds later; the official App's LE reads came later still. A bounded
wake module has now been integrated into production and is present in the
latest neutral artifact. The default cold path is:

```text
stable raw once
  -> on eligible failure, one A2DP ConnectProfile attempt (20 s)
  -> require fresh FDDF exact identity/PID (30 s)
  -> DisconnectProfile and confirm release (5 s)
  -> stable raw retry once
  -> original LE fallback
```

All stages share one `150`-second outer deadline. If profile release is not
confirmed, the controller fails before any role write.

The latest silent no-button hardware run began with no active audio stream, a
clean journal and no resolved ready-made BlueZ device session. START completed
in `122.15` seconds as `accepted_unconfirmed`/`linked`. Status then reported the
exact two-member pair verified/healthy and Aura route `fresh_le`. STOP completed
in `15.89` seconds as `accepted_unconfirmed`/`ready`; the final journal was clean
and the service retained `NRestarts=0`. The phone App did not participate in the
transaction, but no ADB evidence was collected about contemporaneous phone
state.

This verifies the overall production no-button cold path inside the shared
`150`-second deadline. It exercised the `fresh_le` route, not the A2DP
`wake_then_stable` subpath; that subpath remains separately unproven. The result
is control-plane only: there was no active audio stream, `7951` was not observed,
and no acoustic or BASS/ISO claim follows.

The current offline gate totals are `258` library tests plus `8` CLI tests
(`266` main), plus the FIFO private-file helper gate `1/1`.
Audit, dependency-deny, fallback and privacy gates pass; compatibility evidence
mode is complete.

## Final neutral artifact and deployed service

The final neutral rebuild is `8,284,440` bytes. Its required glibc symbol floor
is `GLIBC_2.34`, and its only dynamic library dependencies are `libc` and
`libgcc`. Artifact, installed-file and running-process digests matched; no
digest value is reproduced here.

After restart and one read-only status, user systemd was enabled/active with
`NRestarts=0` and managed state unknown/offline. Its `15`-second restart-idle
sample used `8,828 KiB` RSS, one thread and `15` file descriptors. Average CPU
was `0.0667%` (`1` tick). This is one observed sample, not a peak bound.

The loopback page and status projection were readable. They exposed exactly two
sanitized members, allowlisted channel data, and the bounded `last_action` and
`age_ms` fields. Host/origin checks, CSP and CSRF protections remained enabled;
no raw identity or response object was exposed.

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
| Historical v0.4 JBL accepted `7957` bytes | GATT write ACK | Observed at ATT layer; distinct from the rejected official `7937` attempt |
| Separate JBL Assistant path intends `7957` to enable broadcaster role | Current app request construction | Decompiled control-plane fact; not the captured home flow |
| Aura accepted ON | AA response for Set Device Info | Observed |
| Aura AA token and ACK semantics | Current app command builder and response predicate | Decompiled control-plane fact |
| Official JBL controller places Aura in receiver slot | Live UI transition persisted after activity reopen | Observed control-plane state |
| Aura entered secondary/on state | AA device-info tokens | Observed |
| It was not Linux dual-sink output | Aura A2DP absent; bridge stopped | Verified |
| Both speakers were audible | Human listening confirmation | Verified |
| Standard BASS source was added | Read failed; encrypted access was not proven | Inconclusive |
| JBL was on-air standard broadcast source in the candidate | Separate Assistant intent says broadcaster; no fresh `7951` or ISO capture | Not proven |
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
| Rust exact-member read reports expected pair configured | Two private member identities and fixed roles matched without printing identifiers | Verified read-only on this pair |
| Retained membership proves live linked state | Same two members remained after successful STOP | Refuted |
| Rust rejects an FDDF-window miss before writing | Real STOP preflight found no verified identity; no device write; journal clean | Verified on this host |
| Rust uncertainty survives a process crash | Older timeout panic left a durable pending record; reopen blocked ordinary writes | Verified on this host; timeout bug fixed |
| Graceful teardown failure can be reported as success | Nonzero propagation plus teardown latch | Refuted by release fixture |
| Failed clean directory sync can clear uncertainty on restart | Independent `uncertainty.pending` marker remains authoritative | Refuted by crash-consistency fixture |
| Web accept failure can lose the actor or skip/double shutdown | Actor return plus exactly-once `shutdown_for_exit` on every listener exit | Refuted by release fixture |
| Successful shutdown masks `AcceptFailed` or clears pending | Original accept error is preserved; pending journal remains authoritative | Refuted by release fixture |
| Explicit Rust recovery is identity-bound | Stable public object triggered BlueZ connection; unique connected random object passed exact FDDF PID/stable-identity matching before accepted STOP normalization | Verified on this pair |
| Native Rust start/stop reaches accepted lifecycle | Accepted actions plus managed linked/ready transitions | Verified on this pair |
| Native Rust no-button cold start can repeat | Two rounds; managed transport reported `br_edr` first and ended `le` second | Verified on this pair; not a reliability rate or identity shortcut |
| Persistent native session provides fast normal STOP | Retained-session STOP completed in approximately 0.44 and 0.57 seconds | Verified on this host |
| Approximately 03:45 old-minimal acoustic attempt proves protocol failure | Home Centre issued an automatic STOP in the same experimental window | Invalidated by concurrent-writer contamination |
| EOF-fixed Home-flow-only Rust produced JBL-source two-speaker audibility | START accepted/local linked; after `15` seconds, JBL audible and Aura silent | Refuted on this pair |
| A fixed `10.5`- or `15`-second delay repairs the JBL-source path | The `15`-second negative excludes the original `2`-second delay as the sole cause | Refuted as a demonstrated fix |
| That clean Home-flow-only Rust comparison sent HTTPS `7957` | The tested backend omitted the separate Assistant command | Refuted |
| Rust automatically fails over to v0.4 after failure | Design forbids retry/failover after rejection or uncertainty | Not implemented by design |
| Exact official Aura add used HomeBT V5 `0x2004` | Live App HCI showed AA/V4 `0x3c` instead | Refuted for the captured run |
| Aura accepted alone means the official group is complete | UI retained the JBL as a selectable off tile | Refuted |
| Captured official Home flow validates JBL as source | Android A2DP entered Aura; Aura became PRIMARY and JBL RECEIVER | Refuted; it validates Aura-source direction |
| The App had to remain open to sustain that relay | Association remained while Android A2DP continued to supply Aura | Refuted for this Aura-source run |
| JBL `7937` GATT attempt completed the captured ENTER | ATT returned Write Not Permitted | Refuted for that attempt |
| Successful JBL home-flow ENTER used Wi-Fi | UI time, encrypted `95`-byte TLS record and exact OneWiFiSession call graph align | Correlated cross-evidence; plaintext not decrypted |
| Successful official home flow used `7957` | Captured sequence was Aura AA plus Wi-Fi enter/exit, with no `7957` step | Refuted for this flow |
| Official removal completed both device sides | Aura AA OFF succeeded; encrypted `94`-byte TLS record aligned with Wi-Fi `exitAuracast` | Observed/correlated |
| Exact JBL completion-stage network plaintext was captured | Synchronized capture contained encrypted TLS records only | Not observed; no decryption claim |
| Observed manual gaps are protocol delays | Approximately `25`- and `10`-second gaps were human operation/playback timing | Refuted as a requirement |
| Reintroduced `7957` is part of the same official Home UI sequence | It belongs to the separate Assistant broadcaster state machine | Refuted; directional cross-state-machine composition |
| HTTPS `7957` works after successful pin matching | HTTP `200` carried an `unknown command` device result | Refuted on this firmware |
| Exact GATT `7957` START relays a JBL network source to both speakers | Two STARTs accepted; requested `5%`; user confirmed both twice | Verified for two requested rounds |
| Four JBL GATT write ACKs equal four business successes | No `7951` was observed | Refuted; ATT evidence only |
| Strict GENA succeeded after the narrow firewall rule | Silent START timed out as `jbl_broadcast_result_timed_out` | Refuted on this firmware |
| ACK confirmation is business-confirmed | `accepted_unconfirmed` plus `broadcast_acknowledgement_only`, CLI exit `0` | Refuted by explicit semantics |
| Managed `linked` proves `7951` or audibility | It records the last accepted controller action | Refuted |
| First-round normal STOP passed | `outcome_unknown`, `failure=aura_ack_timeout` | Refuted; explicit recovery succeeded within `13` seconds |
| Post-fix round-two normal STOP returns ready | Playback idle; ordinary STOP accepted/`ready` in approximately `43` seconds | Verified once, no recovery |
| More acoustic rounds are required by the agreed gate | User requested stopping after two successful rounds | Refuted; sound testing stopped |
| No-button cold control completes within the production deadline | START `122.15` s via `fresh_le`; STOP `15.89` s; clean journal; `NRestarts=0` | Verified for one silent hardware round |
| App LE caused the observed deep-standby wake | A2DP reconnect, stored-key auth/encryption and AVDTP Open preceded FDDF by about `2.5` seconds | Refuted |
| Wake module is production-integrated | Default cold path and shared `150`-second deadline are in the neutral artifact; offline gates pass | Verified offline |
| Wake profile release may be assumed | Missing release confirmation fails before role writes | Refuted by design |
| A2DP `wake_then_stable` subpath passed hardware acceptance | Latest run used `fresh_le`; subpath was not hit | Not tested separately |
| Phone state was verified absent during the cold run | Phone App did not participate, but no ADB state evidence was collected | Not claimed |
| Current offline suite passes | `258` library + `8` CLI (`266` main), FIFO private-file helper `1/1`, audit/deny/fallback/privacy/neutral | Verified |
| Compatibility evidence mode is complete | Dedicated compatibility evidence gate passed | Verified offline |
| Final neutral artifact ABI/dependencies | `8,284,440` bytes; `GLIBC_2.34`; only `libc`/`libgcc` | Verified |
| Rebuilt executable is installed and digest-matched | Artifact, installed-file and process digests matched | Verified; digest kept internal |
| Restarted user service remained stable | enabled/active, `NRestarts=0`, managed unknown/offline | Verified at checkpoint |
| Restart-idle resource sample | `15` seconds: `8,828 KiB` RSS, one thread, `15` fds, `0.0667%` average CPU (`1` tick) | Observed sample, not peak bound |
| Loopback status is sanitized and readable | Two members, allowlisted channel, `last_action`, `age_ms`; CSP/CSRF retained | Verified |
| Play Together P0 is complete | Acoustic/start-stop gates plus silent no-button cold pass exist; `7951`, `wake_then_stable` subpath and release gates remain | Not complete |

## Failed or incomplete approaches

- Independent AirPlay and A2DP output produced audible timing offset.
- The approximately 03:45 ENTER plus Aura ON attempt overlapped an automatic
  Home Centre STOP and cannot establish whether that command set is sufficient.
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
7. Will the A2DP `wake_then_stable` subpath be hit and pass a separate hardware
   run, and can `7951` or another stronger business-state signal be observed?

Pull requests with sanitized, reproducible evidence are welcome. Do not attach
raw HCI logs or device identifiers to public issues.
