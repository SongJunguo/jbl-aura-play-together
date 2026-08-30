# Project goal and acceptance order

- Decision date: 2026-08-30
- Active platform: Ubuntu 22.04
- Next platform: Windows 11, after the Ubuntu repository is accepted and moved

## Goal

Build a dependable, private-credential-safe controller for one tested pair:

- JBL Authentics 300
- Harman Kardon Aura Studio 5

The controller should reproduce the useful Play Together behavior without
requiring the phone application for normal daily operation. It should expose
simple `start`, `stop`, and honest `status` behavior; prefer device-reported
two-member configuration for membership identity while keeping live managed
state and acoustic evidence separate; recover only after bounded failures;
and never claim a stronger evidence level than was observed.

The public repository remains limited to this association problem. General
music accounts, Music Assistant, television control, household automation,
private captures, APKs, firmware, and credentials stay outside it.

## Phase 1: finish Ubuntu first

The Ubuntu implementation is complete only when all of the following hold:

1. A clean install on Ubuntu 22.04 can configure runtime-only credentials and
   both speaker identities without editing source code.
2. Read-only JBL LAN discovery, sanitized device status, and
   `getAuraCastGroupInfo` work with TLS certificate/fingerprint pinning.
3. `start` and `stop` operate the tested speaker pair without routine phone-App
   use or repeated physical button presses.
4. The expected private JBL and Aura identities must be present in the
   device-reported membership configuration. That retained configuration is
   not a live-state proof; fresh command/Aura acknowledgements and managed
   session evidence are also required, and release acceptance includes a human
   acoustic check. A transport ACK alone is never enough.
5. Cold discovery tolerates observed FDDF advertising gaps through bounded
   retries and fails before writes when identity is not proven.
6. Existing working groups are adopted instead of needlessly rebuilt. One LAN
   timeout does not trigger destructive Bluetooth recovery.
7. Service startup, shutdown, crash recovery, competing-phone ownership,
   network loss, speaker power cycling, and repeated start/stop behavior have
   documented outcomes and safe bounds.
8. Offline tests, privacy/history scans, and controlled real-hardware tests pass
   without publishing credentials, identifiers, or raw device responses.
9. The daily controller has measured low idle CPU and memory.
10. The preferred release artifact is one Ubuntu executable. Modular source and
    a Python reference/fallback may remain until the Rust implementation reaches
    full behavior and hardware parity.

The working Python/Ubuntu path must not be removed merely because the Rust
candidate compiles. Rust replaces a path only after the same failure cases and
device postconditions pass.

## Phase 2: move to Windows after Ubuntu acceptance

After phase 1 is accepted, the operator will move the repository to Windows 11.
The Windows phase will then:

1. run the same sanitized parser/state fixtures;
2. verify the LAN/mTLS/group client without protocol changes;
3. replace Ubuntu-specific BlueZ/gatttool interaction with a tested WinRT/BLE
   transport while preserving the Ubuntu backend;
4. add Windows credential ACL checks and service/Task Scheduler integration;
5. produce a separate Windows executable;
6. repeat the same device-reported and human acoustic acceptance tests.

Windows work is deliberately deferred until Ubuntu is stable. During phase 1,
portable interfaces are required, but speculative Windows implementation is
not.

## Distribution principle

"Single file" means one user-facing executable for each target operating
system. Internally, protocol encoding, TLS/LAN transport, Bluetooth transport,
state/evidence reduction, configuration, and CLI/service integration remain
separate modules. This gives simple installation without creating an
unmaintainable single source file.
