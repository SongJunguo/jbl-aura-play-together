# Changelog

## 0.4.0 - 2026-08-29

- Resolve the Aura Studio 5 rotating LE address from fresh typed BlueZ D-Bus
  events, requiring FDDF UUID, product ID, and embedded stable identity to
  match before connecting.
- Enable notifications and use the verified Aura AA control path over the live
  random-address bearer, with BR/EDR retained as a compatibility fallback.
- Add two delayed FDDF rescan attempts by default; never substitute a cached RPA
  or same-name device when identity data is absent.
- Add `install-service`, a per-user boot service, restart-on-failure behavior,
  and the simple installed `jbl-aura-link start|stop|status` command.
- Detect idle control-bearer loss and degraded commands, restart the service,
  and normalize uncertain prior roles before publishing `ready`.
- Persist a private PulseAudio restoration snapshot so failed or restarted
  systemd startup cannot silently leave Bluetooth modules unloaded.
- Release vendor control bearers before best-effort classic A2DP restoration,
  avoiding the observed `br-connection-busy` shutdown ordering failure.
- Record two successful no-button cold-control rounds, one safe FDDF-window
  miss, and a listener-confirmed two-speaker audio pass. These results do not
  claim standard BASS, BIG/BIS, or ISO proof.
- Keep CUDA/GPU use at zero and all real addresses, account material, captures,
  APKs, firmware, and private research outside the public repository.

## 0.3.0 - 2026-08-29

- Introduce held JBL/Aura control sessions so repeated `start` and `stop` do not
  depend on reconnecting after the speakers change role.
- Add strict acknowledgement checks, local-only state, address redaction,
  PulseAudio arbitration, and transient user-systemd supervision.
