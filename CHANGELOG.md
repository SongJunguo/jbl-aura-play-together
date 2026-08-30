# Changelog

## 0.5.0-alpha.1 - 2026-08-30

- Add the Rust 1.96 Ubuntu mainline with pinned mTLS JBL control, exact private
  member-ID verification, native BlueZ Aura control, and one executable.
- Keep retained membership configuration separate from managed live state;
  a successful STOP no longer waits for the two configured members to vanish.
- Add closed `start`, `stop`, `status` and explicit `recover-stop --confirm`
  semantics with application/AA acknowledgements and no automatic backend
  failover after an uncertain write.
- Add an owner-only write-ahead uncertainty journal. A real development panic
  preserved pending state across restart and blocked ordinary writes until an
  acknowledged explicit recovery cleared it.
- Preserve graceful-shutdown/teardown failures as a nonzero service exit and
  latch teardown so later cleanup cannot mask an uncertain result.
- Add an independent owner-only `uncertainty.pending` authoritative marker so a
  failed clean-state directory sync still leaves restart closed to ordinary
  mutations until explicit recovery.
- Return the controller actor with every Web accept/serve error and run exactly
  one `shutdown_for_exit` on normal and abnormal listener exit. Device-safety
  errors take precedence; when shutdown succeeds, the original `AcceptFailed`
  remains visible, and a pending journal is never cleared by this path.
- Prefer the exact paired/trusted stable Aura bearer with bounded retries, map
  it only to a unique connected random GATT object carrying the expected FDDF
  PID and stable identity, and retain strict live FDDF LE discovery as fallback.
- Add a loopback-only Web UI and local API on port 8096, single-writer revision
  checks, Host/Origin/CSRF protection, bounded HTTP framing and a hardened
  user-systemd service that remains independent from Music Assistant on 8095.
- Preserve the v0.4 Python/BlueZ controller as an explicitly selected fallback
  rather than mixing half-pair backends inside one transaction.
- Add cross-version owner-only operation/session locks so v0.4 and Rust cannot
  own the speaker pair concurrently; enabling either unit requires explicitly
  disabling the other.
- Record that the approximately 03:45 Rust acoustic attempt was contaminated by
  an automatic Home Centre STOP and cannot support a protocol conclusion.
- Record the EOF-fixed Home-flow-only negative: without `7957`, Aura AA ON and
  JBL Wi-Fi ENTER were accepted/local `linked`, but JBL-only network playback
  after a `15`-second wait still produced no Aura audio. The official Home flow
  instead validated Android A2DP into Aura, with Aura PRIMARY/JBL RECEIVER.
- Reintroduce separate Assistant `setAuracastBroadcast` (`7957`) broadcaster
  semantics for the project's opposite JBL-source direction as an explicit
  cross-state-machine composition, not one official UI sequence. HTTPS returned
  HTTP `200`/`unknown command`; exact GATT `0x002a` supplied the working ACK-only
  transport, with no `7951`.
- Record the first Rust JBL-source acoustic pass: START accepted, Music
  Assistant targeted only JBL at requested `5%`, and the user confirmed both
  speakers. Normal STOP still failed `aura_ack_timeout`/outcome-unknown;
  explicit recovery returned accepted/`ready` within `13` seconds.
- After the fresh-bearer release fix, record round two: service restart and
  phone-Bluetooth release, a second accepted JBL-only requested-`5%` dual-audio
  START, then one ordinary idle STOP accepted/`ready` in approximately
  `43` seconds without recovery. Stop acoustic testing after the two agreed
  successes; keep P0/release open because `7951`, the unhit A2DP
  `wake_then_stable` subpath and remaining release gates are unresolved.
- Add closed `JBL_BROADCAST_CONFIRMATION=ack|gena` semantics, defaulting this
  firmware to ACK-only `accepted_unconfirmed`/
  `broadcast_acknowledgement_only` with CLI exit `0`; only GENA action `33/34`
  may produce business-confirmed acceptance. Managed `linked` remains separate.
- Record that the authorized narrow firewall rule did not repair strict GENA:
  production START still timed out waiting for the broadcast result and was
  normalized through legacy GATT. No `7951` is proven on this firmware.
- Add the deep-standby wake model from HCI ordering: Android BR/EDR A2DP auto
  reconnect, stored-key authentication/encryption, AVDTP Open, then FDDF about
  `2.5` seconds later. Integrate the bounded production cold path: stable raw
  once, one eligible A2DP wake, exact fresh FDDF gate, confirmed profile release,
  one stable retry, then the original LE fallback under one `150`-second outer
  deadline. Missing release confirmation fails before role writes.
- Include the wake path in the latest neutral artifact and pass `258` library
  plus `8` CLI tests (`266` main), FIFO private-file helper `1/1`,
  audit/deny/fallback/privacy/neutral gates and compatibility evidence mode.
- Record the silent no-button cold hardware pass: no active audio stream, clean
  journal, no resolved ready-made BlueZ session; START `122.15` seconds as
  accepted-unconfirmed/linked through `fresh_le`, exact two-member healthy
  status, then STOP `15.89` seconds accepted-unconfirmed/ready with a clean
  journal and `NRestarts=0`. No phone App participated, but no ADB phone-state
  evidence was collected. The overall cold path passed within `150` seconds;
  A2DP `wake_then_stable` was not hit.
- Freeze the final neutral artifact at `8,284,440` bytes with `GLIBC_2.34` and
  only `libc`/`libgcc`; verify artifact, installed-file and running-process
  digests match while retaining the digest value in release-internal evidence.
- Verify the restarted user service enabled/active with `NRestarts=0`. After one
  read-only status, managed state was unknown/offline; a `15`-second restart-idle
  sample measured `8,828 KiB` RSS, one thread, `15` fds and `0.0667%` average
  CPU (`1` tick).
- Complete the readable loopback Web/status projection with two sanitized
  members, allowlisted channel data and `last_action`/`age_ms`, retaining CSP,
  Host/Origin and CSRF protections.

## 0.4.0 - 2026-08-29

- Resolve the Aura Studio 5 rotating LE address from fresh typed BlueZ D-Bus
  events, requiring FDDF UUID, product ID, and embedded stable identity to
  match before connecting.
- Enable notifications and use the verified Aura AA control path over the live
  random-address bearer, with BR/EDR retained as a compatibility fallback.
- Add two delayed FDDF rescan attempts by default; never substitute a cached RPA
  or same-name device when identity data is absent.
- Add `install-service`, a per-user boot service, restart-on-failure behavior,
  and the simple installed `jbl-aura-link start|stop|status` command.
- Detect idle control-bearer loss and degraded commands, restart the service,
  and normalize uncertain prior roles before publishing `ready`.
- Persist a private PulseAudio restoration snapshot so failed or restarted
  systemd startup cannot silently leave Bluetooth modules unloaded.
- Release vendor control bearers before best-effort classic A2DP restoration,
  avoiding the observed `br-connection-busy` shutdown ordering failure.
- Record two successful no-button cold-control rounds, one safe FDDF-window
  miss, and a listener-confirmed two-speaker audio pass. These results do not
  claim standard BASS, BIG/BIS, or ISO proof.
- Keep all real addresses, account material, captures, APKs, firmware, and
  private research outside the public repository.

## 0.3.0 - 2026-08-29

- Introduce held JBL/Aura control sessions so repeated `start` and `stop` do not
  depend on reconnecting after the speakers change role.
- Add strict acknowledgement checks, local-only state, address redaction,
  PulseAudio arbitration, and transient user-systemd supervision.
