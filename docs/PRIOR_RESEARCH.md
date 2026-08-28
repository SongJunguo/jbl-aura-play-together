# Prior open-source research

Search date: 2026-08-28. GitHub code/repository/issue searches found no public
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
documents the same AA/TLV family, `aa00021300` acknowledgement semantics, DFFD
discovery, and legacy TWS tag `0x39`.

It does **not** study Authentics 300, Aura Studio 5, OneOS PL `7957`, token
`0x3c`, standard BASS, or the RECEIVER/BROADCAST discrepancy. It validates the
method and part of the transport family, not our final mechanism.

### partybox-companion

- <https://github.com/jklingberg/partybox-companion>
- inspected commit `027ee65fb6da362a88023cbb043848c4160c313d`

This project has unusually careful reverse-engineering notes with confirmed,
tentative, and open findings separated. Its PartyBox 520 work maps parts of DFFD
and reports a second rotating LE identity that advertises `0x1853` separately
from the DFFD identity. Bluetooth SIG Assigned Numbers define `0x1853` as Common
Audio Service; Public Broadcast Announcement is `0x1856`. The project explicitly
leaves Auracast group commands as an open question.

That separate-identity observation shows DFFD need not be the only LE identity,
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
2. DFFD is a vendor control/discovery advertisement and may coexist with a
   separate Common Audio Service identity; the inspected source does not prove
   that identity also carries a Public Broadcast Announcement.
3. JBL receiver filtering via Harman manufacturer data is publicly reproduced,
   but not yet verified for this exact pair.
4. No inspected repository resolves the Authentics 300 data-plane state or the
   inconclusive Aura BASS read. Static analysis in this project later showed
   that DFFD `RECEIVER(2)` and OneOS `device_status=2` are different enums, so
   their equal raw value is not itself a protocol contradiction.

This repository publishes that unknown instead of hiding it behind a successful
audio demo.
