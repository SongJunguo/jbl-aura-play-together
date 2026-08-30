# Rust LAN and native-control evidence — 2026-08-30

This record covers the uncommitted v0.5 development branch snapshot. It is not
a release claim and contains no household address, Bluetooth address, private
member ID, UUID, certificate, fingerprint value, private path, raw response, or
account material.

## Tested target

- Ubuntu 22.04
- Rust toolchain 1.96.0
- JBL Authentics 300 firmware `26.24.31.50.00`, OneOS `3.1`
- the existing JBL plus Aura Studio 5 Play Together membership was left
  unchanged during the initial read-only phase

## Positive read-only result

The Rust candidate:

1. loaded a client certificate and private key from owner-only files outside
   the repository;
2. connected directly without system proxies or redirects;
3. enforced the configured server DER SHA-256 pin in the OpenSSL handshake
   verification callback;
4. used unauthenticated UPnP on the same IP only after the mTLS client existed,
   and required `hm_product_name` to equal `JBL Authentics 300`;
5. read a sanitized `getDeviceInfo` projection;
6. read `getAuraCastGroupInfo`, matched both private member IDs exactly to their
   configured identity anchors and verified exactly two expected members with
   `disabled=false`;
7. emitted only canonical model/role labels, firmware/OneOS versions and
   allowlisted channels.

The resulting CLI summary reported configuration, TLS pin, LAN and exact-model
checks successful, plus `pair_configuration=ready` for the expected two-member
Play Together membership configuration. No private identity was printed. This
read-only check sent no playback, volume, Bluetooth or group command.

## Negative checks

- A loopback TLS fixture proves that a wrong pin terminates the handshake and
  returns a typed certificate mismatch before an HTTP request is processed.
- Loopback fixtures also prove that redirects are not followed, slow headers
  and drip-fed bodies cannot extend the deadline, and responses over one MiB
  are rejected.
- A complete synthetic mTLS-to-UPnP flow proves that the wrong product model is
  rejected.
- Offline tests reject missing or non-boolean `disabled`, any member count other
  than two, malformed members, missing Aura, duplicate expected members,
  unknown channel text, duplicate configuration keys, broad file permissions,
  symlinks in the final or parent path, and oversized private files.

## Same-window reference comparison

After the strict parser and negative tests were added, the Rust client and the
existing Python reference each performed a fresh read against the same running
device in one bounded comparison window. Their sanitized state tuples matched:

```text
verified=true disabled=false members=2
```

The private IP, certificate paths, certificate fingerprint, member identifiers
and raw responses were neither printed nor stored in this repository. The
comparison was read-only and did not change playback, volume or group state.

## Native lifecycle and recovery result

The later controlled hardware window exercised the Rust whole-pair backend. It
produced the following sanitized results:

1. The first real `stop` met an Aura FDDF advertising-window miss. Identity
   could not be proven, so the backend rejected the action before the first
   device write; the write-ahead journal returned to `clean`.
2. A direct connection to the discovered LE `Device1` failed. A bounded
   classic-connect nudge through the paired-and-trusted stable public object
   caused BlueZ to connect, while the vendor GATT service appeared on the
   unique connected random object. That random object was adopted only after
   its FDDF payload exactly matched the expected PID and embedded stable
   identity. No cached random address or same-name device was accepted.
3. An explicit `recover-stop --confirm` performed its safe diagnosis, verified
   the paired stable identity, mapped it to the exact current FDDF random GATT
   identity, received the required device acknowledgements and ended managed
   `ready`.
4. Native Rust `start` and `stop` were accepted. Two normal `stop` actions
   through the retained session completed in approximately 0.44 and 0.57
   seconds.
5. Two no-button cold `start` rounds were accepted. Managed status reported
   `br_edr` during the first and ended `le` during the second. This proves the
   observed bounded acquisition outcomes on this pair, not that the first
   label bypassed the exact random-object identity gate or establishes a
   reliability rate.

The crash-persistent uncertainty boundary was also exercised. An older timeout
construction caused a real process panic after the pending journal record had
been durably committed. On reopen, the journal still reported an unresolved
action and ordinary mutations remained blocked. The timeout construction was
fixed; the retained pending state was cleared only by the explicit accepted
recovery above.

The approximately 03:45 full-song Rust attempt sent a network stream only to
the JBL, but Home Centre issued an automatic STOP in the same experimental
window. The reported JBL-only audio is therefore contaminated and cannot
refute or validate the old transaction. A later EOF-fixed clean trial used the
Home-flow-only design without `7957`: Aura AA ON and JBL Wi-Fi ENTER were
accepted and managed state became `linked`; after a `15`-second wait, JBL-only
network playback still produced no Aura audio. The captured official Home flow
used the opposite direction—Android A2DP into Aura, Aura PRIMARY and JBL
RECEIVER. The no-`7957` target-direction design is therefore refuted, and the
exact-GATT candidate reintroduces separate Assistant JBL-broadcaster semantics.
Its first START was accepted and JBL-only network playback at requested `5%`
was confirmed on both speakers. Ordinary STOP then failed outcome-unknown on an
Aura ACK timeout; explicit recovery returned accepted/ready within `13` seconds.
After the fresh-bearer release fix, round two repeated the dual-audio START and
completed one ordinary approximately `43`-second STOP accepted/ready without
recovery. Acoustic testing stopped after the two requested successes.

## Build and test result

- rustfmt and Clippy with warnings denied passed;
- cargo-audit reported zero known vulnerabilities; cargo-deny passed advisory,
  license, source and ban policy, with one non-failing build-only `syn` 2/3
  duplicate-version warning;
- the privacy self-test, current-tree scan and unique reachable Git-blob history
  scan passed without exposing candidate values or filenames;
- a fixed-path Bubblewrap build compiled offline with an empty environment and
  passed the release artifact scan;
- the intermediate offline gate passed at that checkpoint, but final test
  totals are pending after the revised transaction changes;
- final neutral ELF is `8,284,440` bytes, requires `GLIBC_2.34`, and dynamically
  links only `libc`/`libgcc`; the installed digest matches the reviewed artifact
  while the digest value remains release-internal;
- final RSS and syscall measurements are likewise pending that freeze.

## Evidence boundary

This proves a bounded, model-checked Rust LAN status path, exact expected-member
configuration verification, native lifecycle actions on the tested pair,
crash-persistent uncertainty and explicit bounded recovery. A controlled STOP
comparison separately proved that membership configuration is not a live
linked-state signal.

It includes one invalidated 03:45 attempt and one EOF-fixed clean negative for
the Home-flow-only no-`7957` target direction: accepted Aura AA ON plus JBL
Wi-Fi ENTER and local `linked`, then JBL-only audio after a `15`-second wait.
That excludes the original `2`-second delay as the sole cause; fixed
`10.5`/`15`-second waits are not repairs. Exact GATT `7957` now supplies two
Rust two-speaker START passes and one post-fix normal STOP pass. Four JBL
GATT writes were ACKed without `7951`; HTTPS `7957` returned HTTP
`200`/`unknown command`. The implementation composes separate Assistant
broadcaster and Aura receiver semantics; ACK remains transport evidence only.
The bounded wake chain is production-integrated in the neutral artifact and all
offline gates pass (`258` library, `8` CLI, `266` main; FIFO private-file helper
`1/1`; audit/deny/fallback/privacy/neutral; compatibility evidence complete).
The latest silent no-button cold run completed
START in `122.15` seconds via `fresh_le` as accepted-unconfirmed/linked and STOP
in `15.89` seconds as accepted-unconfirmed/ready, with a clean final journal and
`NRestarts=0`. The overall cold path is hardware-accepted within `150` seconds;
the A2DP `wake_then_stable` subpath was not hit or separately proven. No phone
App participated, but there is no ADB phone-state evidence.

A later live counterexample showed why NativePair cannot use those projections
as an idempotent predicate: status was managed linked, healthy/linked, transport
`le`, route `fresh_le`, while Aura was silent; START returned idempotent without
a write. Native START/STOP now always execute the backend. Corrective STOP
returned `accepted_unconfirmed` in `46.71` seconds; the first START
rejected-before-send clean in `49.76` seconds; one bounded retry returned
`accepted_unconfirmed` in `48.56` seconds. The player was
idle afterward, so silence was not initially a grouping result. Playback then
resumed; JBL reported `state=playing`, `volume=20`, while the user confirmed
JBL-only audio and a silent Aura. This ACK-only round failed acoustically. It
does not overturn the two historical successes; it demonstrates instability.

The JBL network source was then stopped and the existing Aura A2DP bridge
started. The test player reported JBL idle/volume `20` and Aura playing/volume
`20`. The initial two-speaker impression did not persist; after more than ten
seconds, the user corrected the result to Aura audible and JBL silent. The
transient second sound is possible residual buffering, not an acoustic pass or
audio-sync conclusion. Aura playback and the bridge were stopped; both players
ended idle and the bridge exited. Neither source direction sustained two-speaker
audio in this regression.
Artifact, installed-file and running-process digests matched. After restart and
one read-only status, the enabled/active service had `NRestarts=0` and managed
unknown/offline. Its `15`-second restart-idle sample was `8,828 KiB` RSS, one
thread, `15` fds and `0.0667%` average CPU (`1` tick).
Loopback Web/status exposed two sanitized members, allowlisted channel data,
`last_action`/`age_ms`, with CSP/CSRF intact.
The verified v0.4 Python/BlueZ path remains an
explicitly selected fallback, but the Rust controller does not automatically
fail over or repeat a write after rejection or an uncertain outcome.
