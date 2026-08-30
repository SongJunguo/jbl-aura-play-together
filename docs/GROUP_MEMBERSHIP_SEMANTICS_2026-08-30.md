# `getAuraCastGroupInfo` membership semantics — 2026-08-30

This is a sanitized real-device evidence update for the tested Authentics 300
and Aura Studio 5 pair. It contains no response body, address, member ID,
certificate, fingerprint, path, or account data.

## Controlled comparison

The test began with the JBL API reporting the expected two members,
`disabled=false`. The verified v0.4 held-session backend then:

1. established both control bearers;
2. completed its known START sequence with the expected transport/application
   acknowledgements;
3. completed receiver-first STOP with the exact Aura AA acknowledgement and
   the expected JBL acknowledgements;
4. remained in local `ready` state;
5. released both sessions through normal `shutdown` after the read-only sample.

No music was played and volume was not changed.

After STOP, `getAuraCastGroupInfo` was read immediately and once per second for
15 more seconds. Every response still reported the same two members and
`disabled=false`. The independent `getDeviceAuracastBroadcastInfo` Wi-Fi GET
returned HTTP 200 with a short non-JSON body on this firmware, so it could not
be used as a typed live-state signal.

## Correct interpretation

For this firmware, the two-member response is evidence of a retained Play
Together membership configuration. It is **not** by itself evidence that the
JBL is currently broadcasting, that the Aura is currently receiving, or that
both speakers are audible.

This does not invalidate the membership read. It remains useful to prove that
the configured pair contains exactly the intended private member identities
and is not marked disabled. It must be reported as `pair_configuration`, not as
a live `linked` state.

## State-machine consequence

The controller must keep these evidence dimensions separate:

- membership configuration: pinned mTLS `getAuraCastGroupInfo`, exact private
  member identities, expected models, and `disabled`;
- command acceptance: HTTPS BasicResponse or verified legacy JBL response;
- Aura role acceptance: exact AA acknowledgement on the held bearer;
- managed live state: the current single-writer transaction/session state,
  which becomes unknown after an external App action, restart, or lost bearer;
- acoustic result: a bounded low-volume human check during release acceptance;
- standards evidence: BASS/BASE/BIG/BIS/ISO, still unproven.

START may not return success merely because the membership configuration was
already present. STOP may not wait for that membership to disappear, because
the controlled comparison proves that it can persist after a successful stop.
Until a separate device-reported live role query is reproduced, fresh START and
STOP success requires the relevant application/control acknowledgements plus a
healthy single-writer session; release acceptance additionally requires the
human two-speaker check.

The same experiment exposed an unrelated local lock self-conflict when an outer
CLI synchronously started an installed service whose `ExecStartPre` acquired
the same operation lock. The CLI now releases the lock only for that systemd
start transaction and reacquires it before any lifecycle command; an offline
regression test covers the ordering.
