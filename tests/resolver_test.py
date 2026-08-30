#!/usr/bin/env python3
"""Offline tests for the typed FDDF identity matcher."""

from __future__ import annotations

import sys
import tempfile
import time
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "lib"))

from jbl_aura_session import (  # noqa: E402
    AuraLeResolver,
    Config,
    HARMAN_FDDF_UUID,
    SessionError,
    Supervisor,
)


class Variant:
    """Small dbus-fast Variant stand-in for dependency-free CI."""

    def __init__(self, value: object) -> None:
        self.value = value


def make_config() -> Config:
    return Config(
        adapter="hci0",
        jbl_mac="02:00:00:00:00:01",
        aura_mac="02:00:00:00:00:02",
        aura_device_name="Aura Studio 5",
        aura_transport="le",
        aura_v4_pid="212d",
        jbl_handle="0x002a",
        aura_handle="0x03ea",
        aura_cccd_handle="0x03ed",
        aura_psm="31",
        jbl_mtu=500,
        aura_mtu=500,
        connect_timeout=2.0,
        aura_connect_window=2.0,
        aura_le_scan_window=1.0,
        aura_le_retries=2,
        aura_le_retry_delay=0.0,
        write_timeout=2.0,
        aura_ack_timeout=2.0,
        join_delay=0.0,
        gatttool="gatttool",
        enter_frame="00",
        start_frame="00",
        stop_frame="00",
        exit_frame="00",
        aura_on="00",
        aura_off="00",
    )


def aura_payload() -> bytes:
    payload = bytearray(24)
    payload[0:2] = bytes.fromhex("2d21")
    payload[11:17] = bytes.fromhex("020000000002")
    return bytes(payload)


class AuraResolverTests(unittest.TestCase):
    def test_accepts_random_rpa_with_pid_and_stable_identity(self) -> None:
        resolver = AuraLeResolver(make_config())
        properties = {
            "Address": Variant("02:00:00:00:00:03"),
            "AddressType": Variant("random"),
            "ServiceData": Variant(
                {HARMAN_FDDF_UUID.upper(): Variant(aura_payload())}
            ),
        }
        self.assertEqual(
            resolver.candidate_from_properties(properties),
            "02:00:00:00:00:03",
        )
        self.assertEqual(resolver.identity_matches, 1)

    def test_rejects_public_or_identity_mismatch(self) -> None:
        payload = bytearray(aura_payload())
        payload[16] ^= 0x01
        for address_type, candidate_payload in (
            ("public", aura_payload()),
            ("random", bytes(payload)),
        ):
            resolver = AuraLeResolver(make_config())
            properties = {
                "Address": "02:00:00:00:00:03",
                "AddressType": address_type,
                "ServiceData": {HARMAN_FDDF_UUID: candidate_payload},
            }
            self.assertIsNone(resolver.candidate_from_properties(properties))
            self.assertEqual(resolver.identity_matches, 0)

    def test_rejects_wrong_product_id(self) -> None:
        payload = bytearray(aura_payload())
        payload[0:2] = bytes.fromhex("e320")
        resolver = AuraLeResolver(make_config())
        properties = {
            "Address": "02:00:00:00:00:03",
            "AddressType": "random",
            "ServiceData": {HARMAN_FDDF_UUID: bytes(payload)},
        }
        self.assertIsNone(resolver.candidate_from_properties(properties))

    def test_summary_never_contains_an_address(self) -> None:
        summary = AuraLeResolver(make_config()).summary()
        self.assertNotIn("02:00:00:00:00", summary)

    def test_supervisor_delays_and_retries_before_connecting(self) -> None:
        outcomes: list[str | None] = [None, None, "02:00:00:00:00:03"]
        resolver_count = 0
        connections: list[tuple[str, str]] = []

        class SequenceResolver:
            def __init__(self, outcome: str | None, number: int) -> None:
                self.outcome = outcome
                self.number = number

            def resolve(self, _deadline: float) -> str | None:
                return self.outcome

            def summary(self) -> str:
                return f"fixture={self.number}"

            def absence_hint(self) -> str:
                return "; fixture absence" if self.outcome is None else ""

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            supervisor = Supervisor(
                make_config(), root / "control.sock", root / "state.json", root / "lock"
            )

            def new_resolver() -> SequenceResolver:
                nonlocal resolver_count
                outcome = outcomes[resolver_count]
                resolver_count += 1
                return SequenceResolver(outcome, resolver_count)

            supervisor._new_aura_resolver = new_resolver  # type: ignore[method-assign]
            supervisor._connect_aura_once = (  # type: ignore[method-assign]
                lambda address, transport, _deadline: connections.append(
                    (address, transport)
                )
            )
            supervisor._connect_aura(time.monotonic() + 2.0)

        self.assertEqual(resolver_count, 3)
        self.assertEqual(connections, [("02:00:00:00:00:03", "le")])

    def test_supervisor_reports_all_exhausted_scan_bursts(self) -> None:
        resolver_count = 0

        class EmptyResolver:
            def __init__(self, number: int) -> None:
                self.number = number

            def resolve(self, _deadline: float) -> None:
                return None

            def summary(self) -> str:
                return f"fixture={self.number}"

            def absence_hint(self) -> str:
                return "; fixture absence"

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            supervisor = Supervisor(
                make_config(), root / "control.sock", root / "state.json", root / "lock"
            )

            def new_resolver() -> EmptyResolver:
                nonlocal resolver_count
                resolver_count += 1
                return EmptyResolver(resolver_count)

            supervisor._new_aura_resolver = new_resolver  # type: ignore[method-assign]
            with self.assertRaisesRegex(SessionError, r"scan 3: fixture=3"):
                supervisor._connect_aura(time.monotonic() + 2.0)

        self.assertEqual(resolver_count, 3)


if __name__ == "__main__":
    unittest.main()
