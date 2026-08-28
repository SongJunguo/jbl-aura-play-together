# Security and privacy policy

## Reporting a vulnerability

Please use this repository's private vulnerability-reporting form when it is
available. Do not open a public issue containing credentials, device addresses,
network addresses, captures, app packages, certificates, or account material.

If private reporting is unavailable, open a public issue containing only a
minimal redacted description and ask the maintainer for a private channel.

## Safe diagnostics

The CLI redacts Bluetooth-address-shaped strings from transport failure output.
This is a convenience, not a guarantee that every external tool is safe to
paste publicly. Review all logs manually and replace local identifiers with
role labels before sharing.

Never commit:

- real device configuration;
- Bluetooth/HCI/ISO captures, screenshots, recordings, APKs, or firmware;
- tokens, passwords, cookies, private keys, client certificates, or QR codes;
- household IP addresses, hostnames, usernames, or absolute home paths.

Run `./tests/privacy.sh` before every public contribution.
