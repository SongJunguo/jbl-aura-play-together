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

The persistent supervisor accepts commands only through a local Unix socket
with mode `0600` inside a mode-`0700` runtime directory. Its JSON state and log
files are also user-private and deliberately omit configured device addresses.
The output buffer is bounded, and the implementation has no listener on a TCP
port, cloud API, account token, or GPU/model process.

Bluetooth utilities still need device addresses as local process arguments.
Consequently, a same-user/root process listing or an external systemd diagnostic
can reveal them on the host. Do not paste `ps`, `systemctl status`, a raw session
log, or the private config into a public issue. The local permission boundary is
not a promise that operating-system administrators cannot inspect the process.

The transient fallback and installed per-user systemd unit use
main-process-first termination so the supervisor can attempt the receiver-first
unlink sequence before its transport children are reaped. The installed unit
also persists only the PulseAudio module names and hex-encoded arguments needed
for crash-safe restoration in a mode-0600 state file. It does not store a device
address there. These measures reduce graceful-stop risk; they cannot make power
loss, speaker power-off, or unconditional process killing transactional.

Never commit:

- real device configuration;
- Bluetooth/HCI/ISO captures, screenshots, recordings, APKs, or firmware;
- tokens, passwords, cookies, private keys, client certificates, or QR codes;
- household IP addresses, hostnames, usernames, or absolute home paths.

Run `./tests/privacy.sh` before every public contribution.
