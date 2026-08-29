# Prior open-source research

Search refreshed: 2026-08-29. GitHub code/repository/issue searches found no public
implementation containing the exact `setAuracastBroadcast`, `OneAuracastRole`,
Aura Studio 5 Play Together, or successful `0x3c` sequence for this device pair.
Search indexes are not complete, so this is not a claim that no private or
unindexed research exists.

## Search scope

The search covered GitHub repositories, code, issues, and discussions;
Sourcegraph; GitLab, Gitee, and Codeberg discovery; general web search; and
English, Chinese, French, German, and Russian query variants. Exact probes
included `setAuracastBroadcast`, `OneAuracastRole`,
`GET_DEVICE_AURACAST_BROADCAST_INFO`, `SET_AURACAST_BROADCAST`,
`aa1304003c0101`, and the observed PL frame prefix. No indexed result exposed
this pair's complete sequence. One secondary code index was unavailable during
the search, which is another reason the negative finding is stated narrowly.

## Closest transparent research

### PartyPair

- <https://github.com/louim-lbs/PartyPair>
- inspected commit `cdf3b8f3869b00c5e3e8711d8c532aa6aee7cf12`

This is high-quality, transparent work on two PartyBox 710 speakers. It combines
Android HCI captures, official-app decompilation, and hardware replay. It
documents the same AA/TLV family, `aa00021300` acknowledgement semantics, FDDF
discovery, and legacy TWS tag `0x39`.

It does **not** study Authentics 300, Aura Studio 5, OneOS PL `7957`, token
`0x3c`, standard BASS, or the RECEIVER/BROADCAST discrepancy. It validates the
method and part of the transport family, not our final mechanism.

### partybox-companion

- <https://github.com/jklingberg/partybox-companion>
- inspected commit `027ee65fb6da362a88023cbb043848c4160c313d`

This project has unusually careful reverse-engineering notes with confirmed,
tentative, and open findings separated. Its PartyBox 520 work independently
confirms two findings that are directly relevant to Aura cold reconnect:

- the connectable control identity uses a rotating random LE address, which
  must be taken from a live scan rather than persisted;
- Harman `0xFDDF` Service Data bytes 11 through 16 carry the stable BR/EDR
  address. Its implementation consumes typed BlueZ `Device1.ServiceData`
  events, and its scanner retains the live `BLEDevice` for the subsequent
  connection.

Those offsets exactly match the independently captured Aura Studio 5 payload:
PID `212d` is encoded little-endian at bytes 0-1 and the configured stable
identity is present at bytes 11-16. A typed BlueZ D-Bus probe on the tested
Ubuntu host resolved a fresh Aura RPA from this tuple on 2026-08-29 without a
button press or command write. A later phone-disconnected hardware pass used the
same matcher for the complete CCCD-enable and AA control path; two requested
no-button cold rounds reached the expected managed states, and one linked pass
was confirmed audible on both speakers. One intervening scan saw no FDDF and
failed before writing, so the upstream live-scan rule remains operationally
important.

The upstream document calls `0xFDDF` unregistered; that classification is
incorrect. Bluetooth SIG Assigned Numbers formally assigns `0xFDDF` to Harman
International. The *payload layout* remains vendor-defined and undocumented.

The same project reports a second rotating LE identity that advertises `0x1853`
separately from the FDDF identity. Bluetooth SIG Assigned Numbers define
`0x1853` as Common Audio Service; Public Broadcast Announcement is `0x1856`.
The project explicitly leaves Auracast group commands as an open question.

That separate-identity observation shows FDDF need not be the only LE identity,
but it does not prove that the second identity is a broadcast source. It also
does not explain our cached enum value or the inconclusive Aura BASS read.

### Google Bumble and Collabora's BlueZ demo

- <https://github.com/google/bumble>
- inspected Bumble commit `68f7c244649c19fabdb40d5d5909e6cf1cb3eb9e`
- <https://www.collabora.com/news-and-blog/blog/2026/05/05/bluez-powered-auracast-broadcasting-on-genio-700/>
- inspected Collabora demo commit `9368148a8b7296d352e0be652058c54a62f82294`

Bumble documents real JBL Go 4 transmit and receive interoperability. In
receiver mode, the JBL filters broadcasts unless their advertisement includes
Harman manufacturer ID `87` with a compatibility payload ending in `dffd`.
Collabora independently used that exact quirk with BlueZ 5.86 and PipeWire to
drive JBL Go 4 receivers from a standards-based Linux broadcast source.

This is strong evidence for a standard PBP/BIS audio path plus a vendor
discovery/acceptance gate, rather than a wholly proprietary audio transport. It
was not tested on Authentics 300 or Aura Studio 5, so it remains a related-model
finding.

### BlueZ and Bleak rotating-address guidance

- <https://github.com/bluez/bluez/blob/5.64/doc/adapter-api.txt>
- <https://github.com/hbldh/bleak/discussions/1246>
- <https://github.com/bluez/bluez/issues/2356>

BlueZ 5.64 explicitly states that once a client calls `SetDiscoveryFilter`,
matching device objects are created even for non-discoverable or
non-connectable advertisements. Bleak's maintainer recommends scanning,
matching advertisement content, and passing the resulting live `BLEDevice` to
the client instead of reconnecting by a persisted RPA. A separate 2026 BlueZ
issue demonstrates the same operational boundary: active-scan-then-connect
works for an RPA peer when background connect does not, and the issue remains
present in newer BlueZ versions. Upgrading BlueZ alone is therefore not the
chosen fix.

The local implementation follows the lowest-risk subset of that guidance: it
uses `dbus-fast` to set an LE discovery filter, listens to typed ObjectManager
and Properties signals, validates FDDF PID plus stable identity, then hands the
fresh RPA immediately to the already hardware-verified `gatttool` session. It
does not parse a `bluetoothctl` PTY.

### Harman multi-speaker patent

- <https://patents.google.com/patent/WO2025081468A1/en>

Harman's patent describes a player or app instructing one audio device to act
as primary and another as secondary; the primary then relays audio and settings
to the other devices using Auracast. This closely matches the observed control
shape of JBL `7957` plus Aura AA token `0x3c`. It is architecture-level
corroboration, not a byte-level specification or proof that the tested firmware
implements every patent embodiment.

### OpenLEAudio

- <https://github.com/FajkPes/OpenLEAudio>
- inspected commit `cfeef374d699f9f35d11d320be5f6c2393c80d7b`

Its JBL Tune 780NC static analysis identifies another JBL protocol generation
and a family of Auracast features (`query`, `scan`, `group info`, `group select`,
`subgroup play`, `status`, `high-quality broadcast`, and `password`). The frame
format and product line differ, so those feature IDs are not commands for this
repository. They do show that JBL's apps separate broadcast discovery, group
selection, subgroup playback, quality, and authentication into distinct state.

### OpenJBL and connect-plus

- <https://github.com/NiceDayZc/openjbl>
- inspected OpenJBL commit `e6111667eaab8ee5816345605ac212f8d7d14725`
- <https://github.com/pembem22/connect-plus>
- inspected connect-plus commit `23e091107be88f8d02987006f3056e25f50ee7c8`

OpenJBL documents the AA framing family, ACK layout, Protocol 4, and DFFD as
Harman service data based on static analysis plus limited hardware checks.
connect-plus contains real older-JBL control code and public protocol
discussion. Together they independently support the AA envelope and
`status=0` ACK interpretation. Neither maps Aura token `0x3c`, the OneOS PL
transport, nor Authentics DFFD role bits.

### Auracast Hacker's Toolkit

- <https://github.com/auracast-research/auracast-hackers-toolkit>
- inspected commit `48bb4fd18c3d6d70a38873fa1f93271547433328`

This nRF52840/Zephyr toolkit can passively observe periodic advertisements,
BIGInfo, BIG synchronization, and BIS traffic for Wireshark analysis. It is a
promising way to identify the real broadcaster in a future experiment. It did
not supply any command used here, and its active disruption experiments are out
of scope. No code is copied from it.

## Related JBL local-control projects

### jbl-soundbar-cli

- <https://github.com/k1rnt/jbl-soundbar-cli>
- inspected commit `7d10bdb1ebe0c5a77c9a4c14ebe4580bd3735309`

Useful for OneOS discovery, local HTTPS `httpapi.asp`, mTLS, mDNS, and Authentics
control. No Play Together/Auracast association implementation was found.

### JBL_Soundbar

- <https://github.com/MrBearPresident/JBL_Soundbar>
- inspected commit `7f347ca97922b6680993d4874fbadbc479449608`

Useful Home Assistant integration and local JBL API reference. It does not
implement `7957` plus Aura AA association.

## Standard references

- [BlueZ Broadcast Assistant documentation](https://github.com/bluez/bluez/blob/master/doc/bluetoothctl-assistant.rst)
- [Google Bumble](https://github.com/google/bumble)
- [BlueKitchen BTstack](https://github.com/bluekitchen/btstack)
- [STM32WBA Broadcast Assistant example](https://github.com/stm32-hotspot/STM32WBA-BLE-Audio-Broadcast-Assistant)
- [Bluetooth SIG BASS 1.0.1](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/BASS_v1.0.1/out/en/index-en.html)
- [Bluetooth SIG Assigned Numbers](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Assigned_Numbers/out/en/index-en.html)
- [Authentics firmware release notes](https://support.jbl.com/ca/en/howto/authentics-200-300-500-software-update-release-notes-us/000043744.html)
- [Aura Studio 5 product page](https://www.harmankardon.com/AURA-STUDIO-5.html)

These explain standard BASS and Auracast roles but contain no model-specific
JBL/Harman bridge for this pair.

## Conclusion

The public literature supports four cautious conclusions:

1. AA/TLV is a real, independently reproduced JBL/Harman control family.
2. Harman's FDDF member UUID carries a proprietary discovery payload and may
   coexist with a separate Common Audio Service identity. Authentics DFFD is a
   distinct app-recognized advertisement family; the names must not be treated
   as interchangeable byte-order spellings. The inspected source does not prove
   that identity also carries a Public Broadcast Announcement.
3. JBL receiver filtering via Harman manufacturer data is publicly reproduced,
   but not yet verified for this exact pair.
4. No inspected repository resolves the Authentics 300 data-plane state or the
   inconclusive Aura BASS read. Static analysis in this project later showed
   that DFFD `RECEIVER(2)` and OneOS `device_status=2` are different enums, so
   their equal raw value is not itself a protocol contradiction.

This repository publishes that unknown instead of hiding it behind a successful
audio demo.
