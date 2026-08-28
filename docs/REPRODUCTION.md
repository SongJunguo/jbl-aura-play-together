# Reproduction guide

This guide separates transport replay from acoustic verification. A successful
command only means the relevant bytes were accepted by the transport; the final
pass condition is that both physical speakers reproduce the same JBL source.

## Safety and scope

- Begin at a barely audible volume and use a short, non-sensitive test signal.
- Keep both speakers close enough to hear, but do not publish room recordings.
- Do not upload real configs, raw Bluetooth captures, APKs, account material,
  network addresses, or device identifiers.
- Stop other applications that control either speaker during the experiment.
- This procedure changes speaker association and may disconnect the Aura's
  Linux A2DP profile. `stop` reverses the vendor association and reconnects
  A2DP only if the tool had released it.

## Prerequisites

The verified host used Ubuntu 22.04 and BlueZ 5.64. Install:

```bash
sudo apt install bluez bluez-tools jq xxd
```

Pair and trust each speaker once through `bluetoothctl`. The JBL must expose its
public LE identity and the Aura must expose its stable BR/EDR identity. Install
the example as private configuration outside the repository:

```bash
config_path="${XDG_CONFIG_HOME:-$HOME/.config}/jbl-aura-link/devices.env"
install -Dm600 config/devices.env.example "${config_path}"
```

Replace only the two placeholder addresses.

## Baseline

1. Power on both speakers and wait until their normal idle state is stable.
2. Make sure the JBL can already play from the source you intend to use.
3. Run `./bin/jbl-aura-link doctor` and resolve every blocking result.
4. Play a short signal on the JBL alone at very low volume.
5. Confirm that the Aura is silent. This is the negative-control observation.

Do not continue if both speakers already play at baseline; another controller
or a previous group may still own the association.

## Link trial

Run:

```bash
./bin/jbl-aura-link start
```

The expected terminal result is two JBL write acknowledgements, one Aura write
acknowledgement, and `link sequence completed`. Then play or continue the same
JBL source and listen to each speaker separately.

Record the trial as a pass only when:

- the JBL remains audible;
- the Aura changes from silent to audible;
- no independent Linux audio stream targets the Aura;
- the content transition is consistent on both speakers.

`./bin/jbl-aura-link status` may show `RECEIVER(2)`. That private DFFD field is
diagnostic and does not override the acoustic result or prove the LE Audio
direction.

## Unlink control

Run:

```bash
./bin/jbl-aura-link stop
```

Continue the same JBL source. The expected control result is that JBL remains
audible and Aura becomes silent. If the tool had released Aura A2DP, it also
attempts to restore that profile.

## Repeatability record

For a useful sanitized report, record only:

- speaker models and firmware versions;
- Linux distribution, kernel, BlueZ, and tool version;
- start/stop outcome and whether each transport acknowledged;
- the baseline/link/unlink audible yes/no matrix;
- whether power cycling changed the result.

Replace all Bluetooth and network addresses with role labels before sharing.
Do not interpret a failed or protected BASS read as an empty Receive State.

## Stronger protocol evidence

Identifying which device emits the actual periodic advertisement and BIG/BIS
requires a fresh HCI/ISO-capable capture or a passive LE Audio sniffer. The
current repository has not made that capture, so its verified claim stops at
the vendor command sequence and audible two-speaker result.
