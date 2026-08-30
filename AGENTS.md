# Repository working rules

These rules apply to every automated or human contribution in this repository.

## Project goal and platform order

- Play Together is the unique P0 capability and the active development
  mainline. Do not divert implementation time to EQ, generic playback, source,
  button, mode, or Home Assistant parity until the P0 acceptance gate passes.
- P0 includes identity-safe discovery, start, stop, honest device-reported
  status, cold restart, bounded recovery, service lifecycle, and a minimal UI
  for those operations.
- The broader open-source JBL One replacement remains the product vision, but
  its generic controls are P2 work after Play Together is reliable.
- First deliver a reliable, low-resource Ubuntu 22.04 controller for the exact
  JBL Authentics 300 plus Harman Kardon Aura Studio 5 pair.
- Finish and accept the Ubuntu implementation before spending development time
  on Windows-specific Bluetooth, service, installer, or UI work.
- After the Ubuntu release is stable, the operator will move the repository to
  Windows 11 and continue the port there. Windows is phase two, not a parallel
  v0.5 release blocker.
- Preserve platform-neutral protocol models and transport interfaces during the
  Ubuntu phase so the later Windows port does not require a protocol rewrite.
- The preferred deliverable is one executable per operating system. Source code
  remains split into small, testable modules; "single executable" never means a
  single oversized source file.
- Product behavior, scope, and acceptance are normative in
  `docs/REQUIREMENTS.zh-CN.md`. `docs/PROJECT_GOAL.md` is the shorter project
  goal summary. When the requested behavior changes, update the requirements
  before implementing the change.

## Scope

- Keep the public project limited to interoperability between a JBL Authentics
  300 and a Harman Kardon Aura Studio 5.
- Do not add Music Assistant, QQ Music, account login, television control,
  household orchestration, APKs, firmware, captures, or private research data.
- Treat `Home_Centre` as a private research/deployment workspace. Never copy a
  directory from it wholesale; move reviewed files one at a time.

## Third-party sources and clean-room work

- **Official-App-first gate:** no feature implementation, protocol mutation, or
  live device write may begin until the relevant end-to-end path has been
  traced in the exact supported versions of JBL One and/or Harman Kardon One.
  If any required stage is still unknown, keep the feature in research status
  and close that gap first.
- Treat the authorized JBL One and Harman Kardon One applications as the
  primary behavioral references for this exact Authentics 300 plus Aura Studio
  5 interoperability work. Public GitHub projects are secondary references and
  must not override a model-specific official-App path.
- Before implementing or changing any feature, trace the relevant path through
  both Apps end to end where applicable: product/category configuration, UI or
  ViewModel entry, device-type branch, connection/session selection, command
  construction and serialization, transport/UUID/CCCD use, response matching,
  state update, timeout/retry policy, and teardown. Record which App owns each
  side of a cross-device transaction.
- The mandatory trace for a Bluetooth feature includes advertisement format
  and identity binding, current-address selection, Android connection transport
  and reuse rules, service/characteristic/descriptor discovery, notification
  enablement, MTU behavior, every application frame, transport callback,
  business-response correlation, device-state notification, timeout, retry,
  ownership, and disconnect timing. The corresponding LAN trace must include
  endpoint construction, authentication, encoding, command mapping, response
  parser, state postcondition, timeout, retry, and session lifetime.
- Pin the exact App name and version for every conclusion. When JBL One and
  Harman Kardon One take different branches, follow the App and product
  category that actually owns the controlled device; do not merge branches
  merely because command names look similar.
- When a packet capture from the exact App version and physical device
  conflicts with a reachable static branch, the observed runtime branch is the
  implementation reference. Preserve both findings, explain the dispatch
  condition as unresolved until proven, and never force the static branch onto
  the device merely because its type name appears more specific.
- Historical scripts and successful listening tests are regression evidence,
  not protocol specifications. A historical command may remain as an explicit
  compatibility fallback only after the official path is implemented and the
  deviation is documented.
- Select a command family from the exact product classification and command
  format. A transport write acknowledgement or a generic command success must
  never be used to substitute for the model-specific business response. In
  particular, do not treat a legacy-device command as valid for a HomeBT V5
  device merely because the firmware acknowledges its envelope.
- Keep the complete decompilation trail, cross-references, and raw evidence in
  the private Home Centre research tree. Publish only minimal independently
  expressed interoperability facts, file-free evidence summaries, synthetic
  fixtures, and original implementation code; never publish vendor source,
  APK contents, or private captures.
- For every implemented feature, maintain a private evidence index that points
  to the exact App version and source locations used, then validate the derived
  clean-room behavior with offline golden tests before any bounded hardware
  experiment.
- Each private evidence-index entry must state: supported product identity,
  official caller, connection owner, exact request bytes, exact response and
  success predicate, observed state transition, cleanup path, unresolved
  assumptions, and the synthetic public fixture derived from it. Review that
  entry before authorizing the first hardware write.
- Record every external repository URL, inspected commit, license, and the
  specific fact or design it informed in `docs/PRIOR_RESEARCH.md`.
- Protocol facts, observed request/response shapes, and general architecture
  may inform an independent implementation. Do not copy expressive source code
  from a repository unless its license clearly permits redistribution.
- `MrBearPresident/JBL_Soundbar` currently has no license. Treat its code and
  bundled credentials as reference-only and do not copy either.
- `k1rnt/jbl-soundbar-cli` declares MIT in package metadata but currently lacks
  a license text. Treat its code as reference-only until that is resolved.
- Never copy vendor application or firmware code. Publish only the minimum
  interoperability facts needed for this device pair.
- If licensed third-party code is ever incorporated, preserve attribution and
  license notices and identify the copied files in `NOTICE.md`.

## Credentials and privacy

- Private interoperability work may use credentials, certificates, keys, and
  device identifiers obtained from the two inspected reference projects or
  from devices/applications the operator is authorized to study. This is a
  private-runtime permission only, not permission to redistribute them.
- Keep that private material outside the public checkout in an operator-owned
  runtime directory. Restrict it to the current user with POSIX permissions or
  the equivalent Windows ACL.
- Never commit a certificate, private key, password, token, cookie, APK,
  firmware image, packet capture, real Bluetooth address, household IP,
  hostname, username, absolute home path, group/member identifier, or complete
  device-derived payload containing one of those identifiers.
- Client credentials must be supplied at runtime from files outside the
  repository. Never embed them in source, fixtures, packages, executables,
  containers, CI variables, logs, crash reports, releases, or Git history.
- Do not disable TLS authentication silently. A public LAN client must pin the
  device certificate/fingerprint, or require an explicit insecure opt-in with a
  clear warning.
- Public output is allowlisted and sanitized; raw device JSON stays private.
- Run `tests/privacy.sh` before every commit and inspect the staged diff
  manually. A passing scanner is not permission to publish an unreviewed file.

## Evidence and status semantics

- Keep these evidence levels separate: transport write acknowledgement, vendor
  application response, device-reported two-member topology, human acoustic
  confirmation, and packet-level BASS/BASE/BIG/BIS/ISO proof.
- Never report a link as verified from a GATT write acknowledgement alone.
- Prefer `getAuraCastGroupInfo` reporting both expected model names as the
  machine-readable association check. It still does not prove audibility or
  standards-compliant Auracast.
- Do not use a source-specific player field as global playback state. On the
  tested firmware, UPnP `GetInfoEx` reflects Bluetooth playback more accurately
  than `getPlayerStatus`.
- Unknown firmware fields remain unknown. Parsers must tolerate extra fields
  and reject missing required structure without exposing the raw response.

## Hardware safety and reliability

- Default tests are offline or read-only. Any command that changes grouping,
  source, playback, EQ, mute, or volume requires explicit operator intent.
- Snapshot the current device-reported group before a controlled write test and
  verify the postcondition afterwards. Preserve the existing working group on
  failure whenever possible.
- Volume writes are range-checked. Values above the documented safe default
  require an explicit loudness override.
- A single LAN timeout must not trigger Bluetooth recovery. Use bounded retries
  and a consecutive-failure threshold; never loop indefinitely.
- Do not claim success when a rollback, stop, or reconnect is unverified.

## Firewall and callback exposure

- The operator authorizes a host-firewall change when it is necessary to
  receive the official JBL OneOS `7951` business-result callback. This
  authorization is limited to the Play Together callback path and is not a
  general permission to expose the service or development host.
- Use one documented, fixed TCP callback port. Bind the callback listener only
  to the selected private LAN address, and allow inbound traffic only from the
  privately configured JBL address. Never allow the port from an entire
  subnet, every interface, the public Internet, or an unverified hostname.
- The callback server must additionally enforce the configured JBL source
  address, an unpredictable per-subscription path, the expected GENA SID and
  sequence semantics, the `NOTIFY` method, strict header/body limits, bounded
  XML/JSON parsing, and a short subscription timeout. A firewall rule is not an
  application authentication mechanism.
- Install the narrow firewall rule only after offline parser/listener tests
  pass. Record the exact rollback command, verify the active rule and a real
  callback, and remove the rule if the callback transport is abandoned.
- Do not place private addresses in the repository, generated unit files,
  examples, logs, or release artifacts. Resolve the source address from the
  owner-only runtime configuration when rendering an operator-specific rule.

## Android wireless-debugging evidence

- The operator authorizes using a paired Android phone's wireless debugging,
  ADB inspection, controlled UI automation, and packet capture when static APK
  analysis and device APIs leave a concrete Play Together protocol gap.
- Treat phone-assisted dynamic analysis as the required escalation when static
  decompilation cannot settle actual address/transport selection, serialized
  bytes, callback order, business-response correlation, state notifications,
  or session lifetime. Do not fill those gaps by guessing from a nearby model
  or a generic protocol acknowledgement.
- A bounded official-App reproduction may use Android Bluetooth HCI snoop,
  logcat, dumpsys, developer-mode Bluetooth diagnostics, and operator-approved
  UI automation. Correlate timestamps across UI intent, GATT/L2CAP activity,
  application callbacks, speaker state, and audible outcome before deriving a
  clean-room fixture.
- Prefer static analysis and read-only inspection first. Before any automated
  tap or App action that can change speaker state, take a sanitized topology
  snapshot and apply the same bounded-write and rollback rules as direct tests.
- Keep raw APKs, captures, Android backups, logs, device serials, ADB addresses,
  pairing data, account data, tokens, certificates, and private identifiers in
  the private Home Centre research area with owner-only access. They must never
  enter this public checkout, Git history, CI artifacts, release bundles, issue
  text, or command examples.
- Public evidence may contain only the smallest clean-room protocol fixture or
  conclusion needed for interoperability. Remove personal traffic, stable and
  rotating identifiers, private IPs, timestamps that identify the operator,
  credentials, and unrelated App/account data before publication.
- Do not keep wireless debugging or capture running after the bounded research
  session. Report whether phone/App automation changed speaker state.

## Language and architecture

- Ubuntu 22.04 is the active implementation and acceptance platform. Windows 11
  is the next porting platform after Ubuntu is complete. Keep protocol models,
  command semantics, sanitized fixtures, and state reduction platform-neutral.
- Rust 1.96.0 is the product implementation mainline. Put new protocol models,
  state decisions, CLI behavior and the later embedded Web UI in small typed,
  testable Rust modules that compile into one executable.
- Python 3.10+ is the verified behavioral reference and rollback. Keep its
  existing typed protocol/session modules testable, but do not add a second new
  product state machine there.
- Keep Bash as a thin launcher/installer and for small Linux integration glue;
  do not add new protocol state machines in Bash. PowerShell, if needed, is
  likewise only packaging/service glue.
- Put platform I/O behind interfaces. During phase one, implement and verify the
  Ubuntu LAN and Bluetooth backends. Do not remove the current BlueZ/gatttool
  compatibility path until its replacement passes the same hardware tests.
- Home Assistant integration, if added later, is a thin adapter to the Rust
  local service and must not duplicate the OneOS/Play Together state machine.
- Rust may replace the Python runtime only after the Ubuntu parity gates in
  `docs/ADR-0001-LANGUAGE.md`; Windows parity is a later porting gate, not a
  prerequisite for completing the Ubuntu mainline.
- A Rust build must load credentials at runtime; `include_bytes!` or equivalent
  embedding of a certificate/private key is prohibited.
- Use the exact non-latest stable toolchain pinned in `rust-toolchain.toml`.
  Do not follow the moving `stable` channel. Upgrade only for a relevant
  security fix, a required dependency, or the later Windows port; document the
  reason and rerun the full locked test suite.
- On this development host, inspect `conda env list` before running Python and
  choose the appropriate isolated environment. CI and public setup may use a
  dedicated virtual environment.

## Verification and release

- New protocol behavior needs synthetic unit tests before live testing.
- Before release, run locked Rust formatting, Clippy, tests, RustSec audit,
  dependency/license policy, neutral release build and artifact scan, plus the
  Python/Bash fallback tests, ShellCheck, privacy guard and Git-history scan.
- Live acceptance starts read-only. Controlled writes use the minimum number of
  attempts and record firmware, command, precondition, acknowledgement,
  device-reported postcondition, and whether a person performed an acoustic
  check.
- Release notes must distinguish current behavior from historical experiments
  and must not expose private identifiers.
- A feature is Ubuntu-supported only after the Ubuntu fixture suite and
  real-device postcondition pass. Later, a feature becomes cross-platform only
  after the same checks pass on Windows 11; planned support must never be
  presented as verified support.

## Dependency installation

- Dependencies necessary to complete the documented Ubuntu goal are authorized
  for installation without repeated confirmation. Install only what has a
  concrete build, test, Bluetooth, packaging, or service purpose.
- Keep Python packages in a dedicated environment and never modify Conda base.
  Keep Rust on the repository-pinned rustup toolchain. Prefer project-local or
  user-local tooling when it avoids unnecessary system changes.
- Record required runtime/build packages and their purpose in
  `docs/DEPENDENCIES.md`; pin language dependencies with lock files.
- Do not install Android Studio, an emulator, a desktop stack, or a broad SDK
  when a smaller command-line dependency is sufficient.
- If a required Ubuntu package needs sudo and non-interactive elevation is not
  available, provide the operator one reviewed command to run. Never request,
  store, or reuse a sudo password.
