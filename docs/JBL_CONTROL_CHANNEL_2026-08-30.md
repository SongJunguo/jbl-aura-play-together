# JBL Play Together control-channel decision — 2026-08-30

This is a sanitized clean-room interoperability record for the tested JBL
Authentics 300 firmware. No network address, certificate, fingerprint, device
identifier, account data, APK, capture, or decompiled source is included.

## Decision

Use the device's mTLS HTTPS OneOS API as the primary JBL Play Together control
transport. Keep the verified PL/GATT path as a compatibility fallback. Treat
WebSocket transport as an optional capability for other firmware, not as this
device's main path.

## Evidence

- A read-only port check found HTTPS and UPnP available while TCP port 8080
  explicitly refused connections.
- The sanitized live feature response did not advertise
  `websocket_connect`.
- JBL One 2.7.9 selects its HTTPS Wi-Fi session when that capability is absent.
- The App maps Play Together enter/exit and command `7957` to dedicated OneOS
  Wi-Fi commands; it does not route them through `sendAppController`.
- The App's `sendAppController` model represents remote-control key presses,
  so it is not an evidence-backed carrier for Play Together enter/exit.
- The inspected optional 8080 path is not a reason to call the transport WSS:
  this App build constructs a plain `ws://` URL in the relevant configuration.

The interoperable HTTPS requests are form-encoded POSTs to the pinned mTLS
device endpoint:

```text
command=enterAuracast&payload=null
command=setAuracastBroadcast&payload={"action":1,"broadcast":{...}}
command=setAuracastBroadcast&payload={"action":2}
command=exitAuracast&payload=null
```

The inspected Wi-Fi session constructs the body as the literal
`command=<name>&payload=<compact-json>` string; the JSON is not evidence that a
generic form library may reorder or reinterpret it. The start object uses the
controlled JBL's own upper-case identity and the fixed schema documented in
[Protocol](PROTOCOL.md).

Command acceptance requires a valid BasicResponse with `error_code == 0`.
HTTP status alone is insufficient, and BasicResponse does not prove that the
Aura receives audio. The separate Assistant broadcaster result path
additionally waits for `7951`, which has still not been captured in a
sanitizable Linux trial. A bounded
`getAuraCastGroupInfo` read must also confirm exactly the expected private
member identities and `disabled=false`, but the later controlled STOP
comparison proved that this is a retained membership configuration, not a live
success signal. See
[membership semantics](GROUP_MEMBERSHIP_SEMANTICS_2026-08-30.md).

An approximately 03:45 Rust attempt used `enterAuracast` plus Aura ON without
`7957`, but Home Centre issued an automatic STOP in the same experimental
window. Its JBL-only audio was contaminated by a concurrent writer and cannot
establish whether that command set is sufficient.

A later EOF-fixed clean trial used the Home-flow-only transaction without the
separate Assistant `7957` command. Aura AA ON and JBL Wi-Fi ENTER were accepted
and local state became `linked`. After a `15`-second wait, Music Assistant sent
audio only to JBL; the user explicitly confirmed JBL audio and a silent Aura.
This refutes that design for the JBL-source target and excludes the original
`2`-second delay as the sole cause. Fixed `10.5`/`15`-second waits are not a
demonstrated repair.

## Fallback

The existing persistent PL/GATT backend remains the verified rollback path:

```text
ENTER: 50 4c 01 1f 00 00
EXIT:  50 4c 02 1f 00 00
```

The broadcaster command `7957` is part of the verified historical v0.4
compatibility sequence, and the App maps it to the HTTPS Wi-Fi command
`setAuracastBroadcast` in a separate Auracast Assistant workflow. The
simultaneous Home-flow capture did not use it because that run sent Android
A2DP into Aura, which became PRIMARY while JBL became RECEIVER. The project
target has the opposite direction, so `7957` broadcaster semantics are being
reintroduced deliberately rather than inferred as a Home UI step. Dynamic
testing further showed that HTTPS `setAuracastBroadcast` returned HTTP `200`
with an `unknown command` device result after pin matching; the working START
transport for `7957` is exact GATT value handle `0x002a`.

## Directional JBL-source result

The bounded transaction composes two official state machines:

1. establish and hold the Aura control bearer; snapshot the expected retained
   two-member configuration and current managed-state evidence;
2. one HTTPS `enterAuracast` accepted by BasicResponse;
3. one exact-GATT `7957` write on `0x002a` for JBL broadcaster intent; ATT ACK
   remains transport acceptance only;
4. Aura ON through the exact AA ATT Write Command and correlated success
   notification;
5. re-check the exact intended membership, retain only acknowledged managed
   live state, then perform one low-volume human listening check.

STOP and a bounded rollback remain receiver-first: Aura OFF, separate GATT
broadcast stop, then HTTPS EXIT. ACK/BasicResponse is not promoted to
complete-ring or acoustic proof. A timeout or disconnect after any send remains
outcome-unknown, with no retry and no automatic v0.4 failover.

The simultaneous capture cross-identifies the official Home flow as Wi-Fi
`enterAuracast/exitAuracast` without `7957`, but its audio direction was Aura to
JBL. TLS plaintext was not decrypted; see the
[sanitized runtime evidence](OFFICIAL_APP_RUNTIME_EVIDENCE_2026-08-30.md). The
candidate above is therefore an explicit direction-aware composition of Home/
AA receiver semantics with Assistant broadcaster semantics. It is not claimed
to be one official UI sequence, and no fixed waiting period is part of its
success predicate.

In the first exact-GATT hardware round, START was accepted and Music Assistant
targeted only JBL at the requested `5%`; the user confirmed both speakers. Four
JBL GATT writes received ATT ACKs, but no `7951` arrived. After the authorized
narrow firewall rule was installed, strict SUBSCRIBE-before-GATT START still
ended `jbl_broadcast_result_timed_out` and was normalized through legacy GATT;
the rule is not protocol-success evidence. Ordinary STOP failed
outcome-unknown with `failure=aura_ack_timeout`; explicit `recover-stop`
returned accepted/`ready` within `13` seconds. After the fresh-bearer release
fix, round two restarted the service, released phone Bluetooth ownership,
repeated the accepted requested-`5%` dual-audio START, and completed ordinary
idle STOP as accepted/`ready` in approximately `43` seconds without recovery.
Sound testing stopped after the two agreed successes. P0/release remains
incomplete because `7951`, the unhit A2DP `wake_then_stable` subpath and
remaining gates are open. The overall no-button cold path has one accepted
hardware round through `fresh_le`.

`JBL_BROADCAST_CONFIRMATION=ack|gena` is closed and defaults to `ack` on this
firmware. ACK mode returns `accepted_unconfirmed` with
`broadcast_acknowledgement_only` and CLI exit `0`; strict GENA requires action
`33/34` for `accepted` with `broadcast_business_notification`. Managed `linked`
does not imply either business notification or audibility.
