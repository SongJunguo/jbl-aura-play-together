# Upstream feature intake plan

This document records what may be learned from the two closest JBL local-control
projects without copying unlicensed source or redistributing credentials.

## Source boundary

| Project | Useful as | Source-code intake |
|---|---|---|
| [`MrBearPresident/JBL_Soundbar`](https://github.com/MrBearPresident/JBL_Soundbar), inspected `7f347ca97922b6680993d4874fbadbc479449608` | Home Assistant entity design; OneOS HTTPS and UPnP command inventory | No copying while the repository has no license |
| [`k1rnt/jbl-soundbar-cli`](https://github.com/k1rnt/jbl-soundbar-cli), inspected `7d10bdb1ebe0c5a77c9a4c14ebe4580bd3735309` | Native CLI organization; mDNS, OneOS and UPnP protocol reference | Package metadata says MIT, but the inspected tree lacks the license text; reference-only under this repository's intake gate |

The inspected `JBL_Soundbar` tree bundles credential-named PEM files.
`jbl-soundbar-cli` references client material through compile-time includes,
but that material was not present in the inspected local tree. This repository
imports neither form, compiles neither into an executable, places neither in
CI, and redistributes neither.

The fixed-snapshot audit found one immediate clean-room intake candidate:
bounded `_jbl-product._tcp.local.` discovery. The upstream implementations are
not copied: one automatically selects an unordered first result, neither binds
a candidate to this exact Authentics 300 through pinned mTLS plus UPnP model
verification, and neither has hostile TXT or multi-device tests. Their TLS
verification bypasses, raw logging, soundbar source tokens, legacy EQ models,
toggle-only controls, and UPnP-to-HTTPS write fallback are explicit non-intake
items.

## Features to implement independently

| Capability | Intended clean-room implementation | Authentics 300 rule |
|---|---|---|
| Discovery | `_jbl-product._tcp` mDNS discovery with explicit device selection | Never select solely by friendly name when multiple devices exist |
| Device information | Sanitized OneOS `getDeviceInfo` projection | Return model/firmware fields only; discard identifiers |
| Pair membership | OneOS `getAuraCastGroupInfo` projection | Verify private identities; do not treat retained membership as live state |
| Playback state | UPnP `GetInfoEx` | Prefer this for external Bluetooth playback |
| Volume/mute | UPnP RenderingControl with strict bounds | Volume is limited to `0..9`; absolute volume and mute require the direct-control lock and passed separate real-device readbacks |
| Source/status | Capability-probed OneOS queries | Do not treat `getPlayerStatus` as universal playback truth |
| EQ | Firmware-tolerant parsing of presets and seven-band data | Never force an older bass/treble response struct onto unknown JSON |
| Buttons/commands | Named, allowlisted actions with postcondition reads | No arbitrary command/payload passthrough in the public CLI |
| Home Assistant | Async coordinator and entities backed by the shared library | A separate future integration, not copied from the unlicensed component |
| TLS | Runtime client identity plus server certificate/fingerprint pinning | No bundled key and no silent `CERT_NONE` behavior |
| Recovery | LAN membership plus managed live evidence first, bounded Bluetooth fallback second | Consecutive failures are required before recovery starts |
| Cross-platform BLE | Portable scan/connect/write interface, initially evaluated with Bleak | Windows and Ubuntu must pass the same evidence checks |

The v0.5 association repository should initially take only discovery, sanitized
device information, group verification, TLS pinning, and bounded recovery. A
general JBL control/Home Assistant project can later consume the same library,
but music accounts, household orchestration, and unrelated players remain out
of scope here.

## Acceptance order

1. synthetic parser and error-path tests;
2. privacy and Git-history scans;
3. read-only live discovery and state queries;
4. idempotent, bounded control tests where applicable;
5. Play Together start/stop tests only with explicit operator approval;
6. device-reported two-member identity/configuration check, separate managed
   live evidence, followed by a human acoustic check for release acceptance.

No step upgrades the evidence level of another step. In particular, an HTTP
success or Bluetooth write acknowledgement is not an acoustic or BASS/ISO
result.
