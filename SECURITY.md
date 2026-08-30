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

The historical v0.4 persistent supervisor accepts commands only through a local
Unix socket with mode `0600` inside a mode-`0700` runtime directory. Its JSON
state and log files are user-private and deliberately omit configured device
addresses; v0.4 has no TCP listener.

The Rust alpha instead has a fixed loopback TCP listener on
`127.0.0.1:8096`. It does not listen on a LAN interface by default. The local
HTTP surface enforces Host and Origin checks, CSRF tokens for mutations, a
Content Security Policy, bounded request framing/body sizes and bounded,
sanitized responses. Enabling non-loopback access is outside the default
security model and requires explicit authentication and origin policy. Neither
implementation exposes a cloud API or account token.

Rust crash consistency fails closed. Graceful shutdown propagates teardown
errors through a nonzero process exit, and a teardown latch prevents later
cleanup from masking an uncertain result. The independent owner-only
`uncertainty.pending` marker is authoritative across restart: even if syncing a
new clean state fails, ordinary mutations remain blocked until explicit
recovery. Do not delete or edit this marker to bypass recovery.

The loopback Web owner also fails closed on listener exit. Accept/serve errors
return the controller actor, and normal or abnormal listener termination runs
exactly one `shutdown_for_exit`. A device-safety failure takes precedence; a
successful shutdown does not hide the original `AcceptFailed`, and this exit
path never clears a pending journal.

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
