# JBL Aura Play Together for Linux

[简体中文](README.zh-CN.md)

Unofficial, experimental Linux tooling that sends the vendor control sequence
used to associate a **JBL Authentics 300** with a **Harman Kardon Aura Studio 5**.

The project does one job: link or unlink those two speakers. It does **not**
include a music server, streaming provider, account login, cloud API, phone app,
or audio files.

## Verified result and honest boundary

On 2026-08-28, the sequence in this repository was reproduced on real hardware.
The Aura's Linux A2DP path was disconnected, the JBL received OneOS command
`7957`, the Aura accepted vendor AA Play Together ON, and a listener confirmed
that both speakers produced the same test audio.

This is strong evidence for a working **vendor Play Together path**. It is not
proof of a standards-compliant BASS subscription:

- the Aura Broadcast Receive State probe was inconclusive (BASS requires an
  encrypted read, which the original probe did not prove it had established);
- the JBL DFFD role field read `RECEIVER(2)`, not `BROADCAST(1)`;
- no LE Audio ISO capture was made.

The CLI therefore reports transport acknowledgements and conservative
diagnostics. It never turns a successful GATT write into a claim that BASS is
active. See [Evidence](docs/EVIDENCE.md) and [Protocol](docs/PROTOCOL.md).

## Supported test fingerprint

- JBL Authentics 300, firmware `26.24.31.50.00`
- Harman Kardon Aura Studio 5
- protocol behavior cross-checked against JBL One Android app `2.7.9`
- Ubuntu 22.04 / BlueZ 5.64
- open, single-subgroup request with quality `0`

Other firmware and models are unverified.

## How it works

1. The source audio plays on the JBL through any method the JBL already supports.
2. Linux writes OneOS `ENTER_AURA_CAST` and `SET_AURACAST_BROADCAST` to the JBL
   private PL characteristic.
3. Linux writes AA token `0x3c=ON` to the Aura over ATT on its stable classic
   Bluetooth identity.
4. The speakers coordinate in firmware; the user verifies both acoustically.

The tool does not duplicate audio to two Linux sinks, which avoids the two-clock
delay seen with independent AirPlay/A2DP outputs.

## Quick start

Install the small runtime dependency set:

```bash
sudo apt install bluez bluez-tools jq xxd
```

Pair and trust both speakers with `bluetoothctl`, then:

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
# Replace both placeholder MAC addresses in "${config_path}".

./bin/jbl-aura-link doctor
./bin/jbl-aura-link start
./bin/jbl-aura-link status
```

Start audio on the JBL and listen to both speakers. To unlink:

```bash
./bin/jbl-aura-link stop
```

The real device config lives outside the repository by default. Never put
device addresses, captures, certificates, account tokens, or app packages in
an issue or commit.

## Commands

| Command | Purpose |
|---|---|
| `doctor` | Check tools, adapter, config and pairing |
| `start` | Send the vendor association sequence |
| `stop` | Send the vendor unlink sequence |
| `status` | Decode the non-authoritative JBL DFFD role conservatively |
| `frame` | Build PL frames offline without touching hardware |

`start` and `stop` use a process lock, timeouts, strict success-text matching,
and rollback on incomplete start. If Linux owned the Aura A2DP profile, `start`
releases it and `stop` restores it.

## Documentation

- [Reproduction guide](docs/REPRODUCTION.md)
- [Protocol notes](docs/PROTOCOL.md)
- [Evidence and unresolved questions](docs/EVIDENCE.md)
- [Prior open-source research](docs/PRIOR_RESEARCH.md)
- [Security policy](SECURITY.md)

## Resource use

The implementation is Bash + BlueZ GATT control. It has no CUDA, GPU, audio
decoding, or model dependency.

## License and trademarks

Original code and documentation are MIT licensed. This is an independent
interoperability project, not affiliated with or endorsed by JBL, Harman Kardon,
or their owners. Product names are used only to identify compatibility.
