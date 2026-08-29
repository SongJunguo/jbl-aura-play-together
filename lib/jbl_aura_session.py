#!/usr/bin/env python3
"""Persistent, local-only control session for JBL/Aura Play Together.

The speaker role transition can make both control identities reject a fresh
connection.  This supervisor connects to both devices before changing either
role and keeps those two gatttool sessions until an explicit shutdown.

The persistent control path uses the Python standard library.  Fully automatic
Aura cold discovery additionally uses ``dbus-fast`` to consume BlueZ's typed
D-Bus advertisement events instead of scraping ``bluetoothctl`` terminal text.
Device addresses arrive through the process environment, are never written to
the state file, and are redacted from diagnostics.
"""

from __future__ import annotations

import argparse
import asyncio
import dataclasses
import fcntl
import json
import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Pattern


MAC_RE = re.compile(r"(?i)(?:[0-9a-f]{2}:){5}[0-9a-f]{2}")
ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
WRITE_ACK_RE = re.compile(r"Characteristic value was written successfully", re.I)
CONNECT_ACK_RE = re.compile(r"Connection successful", re.I)
MTU_ACK_RE = re.compile(r"MTU was exchanged successfully:\s*([0-9]+)", re.I)
AURA_ACK_RE = re.compile(
    r"Notification handle\s*=\s*0x03ec\s+value:\s*aa\s+00\s+02\s+13\s+00",
    re.I,
)
BASIC_OK_HEX_RE = re.compile(
    r"7b\s+22\s+65\s+72\s+72\s+6f\s+72\s+5f\s+63\s+6f\s+64\s+65"
    r"\s+22\s+3a\s+22\s+30\s+22\s+7d",
    re.I,
)
TRANSPORT_FAILURE_RE = re.compile(
    r"Command Failed|Host is down|Function not implemented|Connection refused|"
    r"Disconnected|connect failed|No route to host",
    re.I,
)
DISCONNECT_EVENT_RE = re.compile(
    r"\bDisconnected\b|Connection (?:closed|terminated|lost)", re.I
)
MAX_BUFFER_CHARS = 262_144
BLUEZ_BUS = "org.bluez"
BLUEZ_ADAPTER_INTERFACE = "org.bluez.Adapter1"
BLUEZ_DEVICE_INTERFACE = "org.bluez.Device1"
DBUS_OBJECT_MANAGER_INTERFACE = "org.freedesktop.DBus.ObjectManager"
DBUS_PROPERTIES_INTERFACE = "org.freedesktop.DBus.Properties"
HARMAN_FDDF_UUID = "0000fddf-0000-1000-8000-00805f9b34fb"


class SessionError(RuntimeError):
    """A controlled transport or state-machine failure."""


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def log(message: str) -> None:
    print(f"[jbl-aura-session] {message}", flush=True)


def env_required(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise SessionError(f"missing environment setting: {name}")
    return value


def env_float(name: str, default: float) -> float:
    raw = os.environ.get(name)
    value = default if raw is None else float(raw)
    if value <= 0:
        raise SessionError(f"{name} must be greater than zero")
    return value


def env_nonnegative_float(name: str, default: float) -> float:
    raw = os.environ.get(name)
    value = default if raw is None else float(raw)
    if value < 0:
        raise SessionError(f"{name} must be non-negative")
    return value


def env_nonnegative_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    value = default if raw is None else int(raw)
    if value < 0:
        raise SessionError(f"{name} must be a non-negative integer")
    return value


def validate_hex(name: str, value: str) -> str:
    if not value or len(value) % 2 or re.fullmatch(r"[0-9a-fA-F]+", value) is None:
        raise SessionError(f"{name} must be even-length hexadecimal")
    return value.lower()


@dataclasses.dataclass(frozen=True)
class Config:
    adapter: str
    jbl_mac: str
    aura_mac: str
    aura_device_name: str
    aura_transport: str
    aura_v4_pid: str
    jbl_handle: str
    aura_handle: str
    aura_cccd_handle: str
    aura_psm: str
    jbl_mtu: int
    connect_timeout: float
    aura_connect_window: float
    aura_le_scan_window: float
    aura_le_retries: int
    aura_le_retry_delay: float
    write_timeout: float
    aura_ack_timeout: float
    join_delay: float
    gatttool: str
    enter_frame: str
    start_frame: str
    stop_frame: str
    exit_frame: str
    aura_on: str
    aura_off: str

    @classmethod
    def from_env(cls) -> "Config":
        jbl_mac = env_required("JBL_BT_MAC").upper()
        aura_mac = env_required("AURA_BT_MAC").upper()
        for name, value in (("JBL_BT_MAC", jbl_mac), ("AURA_BT_MAC", aura_mac)):
            if MAC_RE.fullmatch(value) is None:
                raise SessionError(f"invalid {name}")
        mtu = int(os.environ.get("JBL_GATT_MTU", "500"))
        if mtu <= 3:
            raise SessionError("JBL_GATT_MTU must be greater than 3")
        aura_transport = os.environ.get(
            "AURA_TRANSPORT_OVERRIDE", os.environ.get("AURA_TRANSPORT", "auto")
        ).lower()
        if aura_transport not in {"auto", "bredr", "le"}:
            raise SessionError("AURA_TRANSPORT must be auto, bredr, or le")
        aura_v4_pid = validate_hex(
            "AURA_V4_PID", os.environ.get("AURA_V4_PID", "212d")
        )
        if len(aura_v4_pid) != 4:
            raise SessionError("AURA_V4_PID must contain exactly two bytes")
        return cls(
            adapter=os.environ.get("BT_ADAPTER", "hci0"),
            jbl_mac=jbl_mac,
            aura_mac=aura_mac,
            aura_device_name=os.environ.get("AURA_DEVICE_NAME", "Aura Studio 5"),
            aura_transport=aura_transport,
            aura_v4_pid=aura_v4_pid,
            jbl_handle=os.environ.get("JBL_GATT_HANDLE", "0x002a"),
            aura_handle=os.environ.get("AURA_GATT_HANDLE", "0x03ea"),
            aura_cccd_handle=os.environ.get("AURA_GATT_CCCD_HANDLE", "0x03ed"),
            aura_psm=os.environ.get("AURA_GATT_PSM", "31"),
            jbl_mtu=mtu,
            connect_timeout=env_float("SESSION_CONNECT_TIMEOUT", 18.0),
            aura_connect_window=env_float("SESSION_AURA_CONNECT_WINDOW", 150.0),
            aura_le_scan_window=env_float("SESSION_AURA_LE_SCAN_WINDOW", 30.0),
            aura_le_retries=env_nonnegative_int("SESSION_AURA_LE_RETRIES", 2),
            aura_le_retry_delay=env_nonnegative_float(
                "SESSION_AURA_LE_RETRY_DELAY", 15.0
            ),
            write_timeout=env_float("SESSION_WRITE_TIMEOUT", 8.0),
            aura_ack_timeout=env_float("SESSION_AURA_ACK_TIMEOUT", 8.0),
            join_delay=float(os.environ.get("AURA_JOIN_DELAY", "2")),
            gatttool=os.environ.get("GATTTOOL_BIN", "gatttool"),
            enter_frame=validate_hex("JBL_ENTER_FRAME", env_required("JBL_ENTER_FRAME")),
            start_frame=validate_hex("JBL_START_FRAME", env_required("JBL_START_FRAME")),
            stop_frame=validate_hex("JBL_STOP_FRAME", env_required("JBL_STOP_FRAME")),
            exit_frame=validate_hex("JBL_EXIT_FRAME", env_required("JBL_EXIT_FRAME")),
            aura_on=validate_hex("AURA_ON_FRAME", env_required("AURA_ON_FRAME")),
            aura_off=validate_hex("AURA_OFF_FRAME", env_required("AURA_OFF_FRAME")),
        )

    def redactions(self) -> tuple[str, ...]:
        values = [
            self.jbl_mac,
            self.aura_mac,
            self.jbl_mac.replace(":", "").encode().hex(),
            self.aura_mac.replace(":", "").encode().hex(),
            self.start_frame,
        ]
        return tuple(value.lower() for value in values if value)

    def v4_payload_matches(self, payload: bytes) -> bool:
        if len(payload) < 17:
            return False
        advertised_pid = bytes.fromhex(self.aura_v4_pid)[::-1]
        stable_address = bytes.fromhex(self.aura_mac.replace(":", ""))
        return payload[:2] == advertised_pid and payload[11:17] == stable_address


class AuraLeResolver:
    """Resolve the Aura's current RPA from typed BlueZ D-Bus events.

    The Aura rotates its connectable LE address, so a cached address is not an
    identity.  A candidate is accepted only when a *fresh discovery signal*
    carries Harman FDDF service data with both the configured product ID and
    the configured stable BR/EDR address at the hardware-observed offsets.
    """

    def __init__(self, config: Config) -> None:
        self.config = config
        self.advertisement_events = 0
        self.service_data_events = 0
        self.identity_matches = 0
        self.filter_successes = 0
        self.interfaces_added = 0
        self.properties_changed = 0
        self.random_candidates = 0

    def summary(self) -> str:
        return (
            f"advertisements={self.advertisement_events}, "
            f"service_data={self.service_data_events}, "
            f"identity_matches={self.identity_matches}, "
            f"random_candidates={self.random_candidates}, "
            f"interfaces_added={self.interfaces_added}, "
            f"properties_changed={self.properties_changed}, "
            f"filters={self.filter_successes}"
        )

    def absence_hint(self) -> str:
        if self.advertisement_events > 0 and self.service_data_events == 0:
            return (
                "; Aura FDDF was not observed; the speaker may be connected "
                "to another host or outside its control-advertising state"
            )
        return ""

    @staticmethod
    def _unwrap(value: object) -> object:
        """Unwrap dbus-fast Variants without importing it in offline tests."""
        return getattr(value, "value", value)

    @classmethod
    def _plain_properties(cls, properties: object) -> dict[str, object]:
        if not isinstance(properties, dict):
            return {}
        return {str(key): cls._unwrap(value) for key, value in properties.items()}

    @classmethod
    def _service_data(cls, value: object) -> dict[str, bytes]:
        value = cls._unwrap(value)
        if not isinstance(value, dict):
            return {}
        decoded: dict[str, bytes] = {}
        for key, raw in value.items():
            raw = cls._unwrap(raw)
            try:
                decoded[str(key).lower()] = bytes(raw)  # type: ignore[arg-type]
            except (TypeError, ValueError):
                continue
        return decoded

    def candidate_from_properties(self, properties: dict[str, object]) -> str | None:
        """Return a verified current RPA, or ``None`` for an unrelated object."""
        address = str(self._unwrap(properties.get("Address", ""))).upper()
        address_type = str(self._unwrap(properties.get("AddressType", ""))).lower()
        if MAC_RE.fullmatch(address) is None or address_type != "random":
            return None
        if address == self.config.aura_mac:
            return None
        self.random_candidates += 1
        service_data = self._service_data(properties.get("ServiceData", {}))
        payload = service_data.get(HARMAN_FDDF_UUID)
        if payload is None:
            return None
        self.service_data_events += 1
        if not self.config.v4_payload_matches(payload):
            return None
        self.identity_matches += 1
        return address

    async def _resolve_async(self, timeout: float) -> str | None:
        try:
            from dbus_fast import BusType, DBusError, Variant
            from dbus_fast.aio import MessageBus
        except ImportError as error:
            raise SessionError(
                "Aura LE discovery requires dbus-fast in PYTHON_BIN; "
                "install requirements-le.txt"
            ) from error

        adapter_path = f"/org/bluez/{self.config.adapter}"
        bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
        found: asyncio.Future[str] = asyncio.get_running_loop().create_future()
        device_cache: dict[str, dict[str, object]] = {}
        property_subscriptions: list[tuple[object, object]] = []
        watch_tasks: set[asyncio.Task[None]] = set()
        scanning = False
        discovery_started = False

        async def proxy(path: str) -> object:
            introspection = await bus.introspect(BLUEZ_BUS, path)
            return bus.get_proxy_object(BLUEZ_BUS, path, introspection)

        def maybe_resolve(path: str) -> None:
            if not scanning or found.done():
                return
            self.advertisement_events += 1
            candidate = self.candidate_from_properties(device_cache.get(path, {}))
            if candidate is not None:
                found.set_result(candidate)

        async def watch_device(path: str) -> None:
            try:
                device_object = await proxy(path)
                properties_interface = device_object.get_interface(
                    DBUS_PROPERTIES_INTERFACE
                )
            except (DBusError, KeyError):
                return

            def on_properties_changed(
                interface_name: str,
                changed: dict[str, object],
                invalidated: list[str],
            ) -> None:
                if interface_name != BLUEZ_DEVICE_INTERFACE:
                    return
                self.properties_changed += 1
                cached = device_cache.setdefault(path, {})
                cached.update(self._plain_properties(changed))
                for name in invalidated:
                    cached.pop(str(name), None)
                maybe_resolve(path)

            properties_interface.on_properties_changed(on_properties_changed)
            property_subscriptions.append((properties_interface, on_properties_changed))

        try:
            manager_object = await proxy("/")
            object_manager = manager_object.get_interface(DBUS_OBJECT_MANAGER_INTERFACE)

            def on_interfaces_added(
                path: str, interfaces: dict[str, dict[str, object]]
            ) -> None:
                raw_properties = interfaces.get(BLUEZ_DEVICE_INTERFACE)
                if raw_properties is None:
                    return
                self.interfaces_added += 1
                device_cache[path] = self._plain_properties(raw_properties)
                maybe_resolve(path)
                task = asyncio.create_task(watch_device(path))
                watch_tasks.add(task)
                task.add_done_callback(watch_tasks.discard)

            object_manager.on_interfaces_added(on_interfaces_added)
            existing = await object_manager.call_get_managed_objects()
            for path, interfaces in existing.items():
                raw_properties = interfaces.get(BLUEZ_DEVICE_INTERFACE)
                if raw_properties is None:
                    continue
                device_cache[path] = self._plain_properties(raw_properties)
                await watch_device(path)

            adapter_object = await proxy(adapter_path)
            adapter = adapter_object.get_interface(BLUEZ_ADAPTER_INTERFACE)
            discovery_filter = {
                "Transport": Variant("s", "le"),
                "DuplicateData": Variant("b", True),
                # An empty Pattern matches every device and explicitly bypasses
                # discoverability filtering on BlueZ 5.64.
                "Pattern": Variant("s", ""),
            }
            await adapter.call_set_discovery_filter(discovery_filter)
            self.filter_successes += 1
            # BlueZ may emit InterfacesAdded while StartDiscovery itself is
            # still awaiting its D-Bus reply.  Arm the fresh-event gate first
            # so that short FDDF bursts in that interval are not discarded.
            scanning = True
            try:
                await adapter.call_start_discovery()
                discovery_started = True
            except DBusError as error:
                if "InProgress" not in error.type:
                    raise
            try:
                return await asyncio.wait_for(found, timeout=timeout)
            except TimeoutError:
                return None
            finally:
                scanning = False
                if discovery_started:
                    try:
                        await adapter.call_stop_discovery()
                    except DBusError:
                        pass
                object_manager.off_interfaces_added(on_interfaces_added)
        except DBusError as error:
            error_name = getattr(error, "type", "org.bluez.Error.Failed")
            raise SessionError(f"BlueZ D-Bus LE discovery failed: {error_name}") from error
        finally:
            for properties_interface, handler in property_subscriptions:
                try:
                    properties_interface.off_properties_changed(handler)
                except (AttributeError, ValueError):
                    pass
            for task in watch_tasks:
                task.cancel()
            if watch_tasks:
                await asyncio.gather(*watch_tasks, return_exceptions=True)
            bus.disconnect()

    def resolve(self, deadline: float) -> str | None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        try:
            return asyncio.run(self._resolve_async(remaining))
        except SessionError:
            raise
        except (OSError, RuntimeError, ValueError) as error:
            raise SessionError("BlueZ D-Bus LE discovery failed") from error


class GattSession:
    def __init__(self, name: str, argv: list[str], redactions: tuple[str, ...]) -> None:
        self.name = name
        self.argv = argv
        self.redactions = redactions
        self.process: subprocess.Popen[bytes] | None = None
        self.buffer = ""
        self.buffer_offset = 0
        self.condition = threading.Condition()
        self.command_lock = threading.Lock()
        self.reader: threading.Thread | None = None
        self.connected = False

    def _redact(self, value: str) -> str:
        value = ANSI_RE.sub("", value)
        value = MAC_RE.sub("<bluetooth-address>", value)
        lowered = value.lower()
        for secret in self.redactions:
            if secret in lowered:
                pattern = re.compile(re.escape(secret), re.I)
                value = pattern.sub("<redacted-frame>", value)
                lowered = value.lower()
        return value

    def start(self) -> None:
        if self.process is not None:
            return
        self.process = subprocess.Popen(
            self.argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            bufsize=0,
            close_fds=True,
        )
        self.reader = threading.Thread(target=self._read_output, daemon=True)
        self.reader.start()

    def _read_output(self) -> None:
        assert self.process is not None and self.process.stdout is not None
        fd = self.process.stdout.fileno()
        try:
            while True:
                chunk = os.read(fd, 4096)
                if not chunk:
                    break
                text = chunk.decode("utf-8", errors="replace")
                with self.condition:
                    self.buffer += text
                    if DISCONNECT_EVENT_RE.search(text) is not None:
                        self.connected = False
                    if len(self.buffer) > MAX_BUFFER_CHARS:
                        discarded = len(self.buffer) - MAX_BUFFER_CHARS
                        self.buffer = self.buffer[discarded:]
                        self.buffer_offset += discarded
                    self.condition.notify_all()
        finally:
            with self.condition:
                self.connected = False
                self.condition.notify_all()

    def _tail(self, start: int) -> str:
        relative_start = max(0, start - self.buffer_offset)
        return self._redact(self.buffer[relative_start:][-600:]).strip()

    def _wait(self, pattern: Pattern[str], start: int, timeout: float) -> str:
        deadline = time.monotonic() + timeout
        with self.condition:
            while True:
                if start < self.buffer_offset:
                    raise SessionError(
                        f"{self.name} produced too much output while waiting for a reply"
                    )
                current = self.buffer[start - self.buffer_offset :]
                match = pattern.search(current)
                if match is not None:
                    return match.group(0)
                failure = TRANSPORT_FAILURE_RE.search(current)
                if failure is not None:
                    raise SessionError(
                        f"{self.name} transport failed: {failure.group(0)}; "
                        f"output={self._tail(start)!r}"
                    )
                if self.process is None or self.process.poll() is not None:
                    raise SessionError(
                        f"{self.name} control process exited; output={self._tail(start)!r}"
                    )
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise SessionError(
                        f"{self.name} timed out waiting for {pattern.pattern!r}; "
                        f"output={self._tail(start)!r}"
                    )
                self.condition.wait(min(remaining, 0.25))

    def command(
        self,
        command: str,
        expected: Pattern[str],
        timeout: float,
        second_expected: Pattern[str] | None = None,
        second_timeout: float | None = None,
    ) -> None:
        with self.command_lock:
            if self.process is None or self.process.stdin is None:
                raise SessionError(f"{self.name} control process is not running")
            with self.condition:
                start = self.buffer_offset + len(self.buffer)
            try:
                self.process.stdin.write((command + "\n").encode("ascii"))
                self.process.stdin.flush()
            except (BrokenPipeError, OSError) as error:
                raise SessionError(f"{self.name} control pipe failed") from error
            self._wait(expected, start, timeout)
            if second_expected is not None:
                self._wait(second_expected, start, second_timeout or timeout)

    def connect(self, timeout: float) -> None:
        with self.condition:
            self.connected = True
        try:
            self.command("connect", CONNECT_ACK_RE, timeout)
        except SessionError:
            with self.condition:
                self.connected = False
            raise

    def is_connected(self) -> bool:
        with self.condition:
            return bool(
                self.connected
                and self.process is not None
                and self.process.poll() is None
            )

    def set_mtu(self, mtu: int, timeout: float) -> None:
        self.command(f"mtu {mtu}", MTU_ACK_RE, timeout)

    def write(self, handle: str, frame: str, timeout: float) -> None:
        self.command(f"char-write-req {handle} {frame}", WRITE_ACK_RE, timeout)

    def write_basic(self, handle: str, frame: str, timeout: float) -> None:
        self.command(
            f"char-write-req {handle} {frame}",
            WRITE_ACK_RE,
            timeout,
            BASIC_OK_HEX_RE,
            timeout,
        )

    def write_aura(self, handle: str, frame: str, timeout: float, ack_timeout: float) -> None:
        self.command(
            f"char-write-req {handle} {frame}",
            WRITE_ACK_RE,
            timeout,
            AURA_ACK_RE,
            ack_timeout,
        )

    def close(self) -> None:
        process = self.process
        if process is None:
            return
        try:
            if process.stdin is not None and process.poll() is None:
                process.stdin.write(b"exit\n")
                process.stdin.flush()
                process.wait(timeout=2)
        except (BrokenPipeError, OSError, subprocess.TimeoutExpired):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)
        finally:
            with self.condition:
                self.connected = False
            self.process = None


class Supervisor:
    def __init__(self, config: Config, socket_path: Path, state_path: Path, lock_path: Path) -> None:
        self.config = config
        self.socket_path = socket_path
        self.state_path = state_path
        self.lock_path = lock_path
        self.state = "initializing"
        self.last_error: str | None = None
        self.running = True
        self.shutdown_requested = False
        self.server: socket.socket | None = None
        self.lock_fd: int | None = None
        self.aura: GattSession | None = None
        self.jbl: GattSession | None = None
        self.aura_transport = "unresolved"

    def _previous_state_requires_normalization(self) -> bool:
        if self.state_path.is_symlink() or not self.state_path.is_file():
            return False
        try:
            payload = json.loads(self.state_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError):
            return False
        if not isinstance(payload, dict):
            return False
        return payload.get("state") in {
            "linked",
            "starting",
            "stopping",
            "degraded",
            "failed",
        }

    def _control_sessions_connected(self) -> bool:
        return bool(
            self.aura is not None
            and self.jbl is not None
            and self.aura.is_connected()
            and self.jbl.is_connected()
        )

    def _safe_parent(self, path: Path) -> None:
        parent = path.parent
        if parent.is_symlink():
            raise SessionError(f"refusing symlinked runtime directory: {parent}")
        parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(parent, 0o700)

    def _acquire_lock(self) -> None:
        self._safe_parent(self.lock_path)
        if self.lock_path.is_symlink():
            raise SessionError("refusing symlinked session lock")
        flags = os.O_CREAT | os.O_RDWR
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        self.lock_fd = os.open(self.lock_path, flags, 0o600)
        try:
            fcntl.flock(self.lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise SessionError("another persistent session is already running") from error

    def _write_state(self) -> None:
        self._safe_parent(self.state_path)
        payload = {
            "state": self.state,
            "pid": os.getpid(),
            "updated": now_iso(),
            "last_error": self.last_error,
            "aura_transport": self.aura_transport,
            "evidence": "control acknowledgements only; not BASS/ISO proof",
        }
        temporary = self.state_path.with_suffix(".tmp")
        if self.state_path.is_symlink() or temporary.is_symlink():
            raise SessionError("refusing symlinked session state file")
        flags = os.O_CREAT | os.O_WRONLY | os.O_TRUNC
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        fd = os.open(temporary, flags, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(payload, stream, ensure_ascii=True, sort_keys=True)
            stream.write("\n")
        os.chmod(temporary, 0o600)
        os.replace(temporary, self.state_path)

    def _set_state(self, state: str, error: str | None = None) -> None:
        self.state = state
        self.last_error = error
        self._write_state()
        log(f"state={state}" + (f" error={error}" if error else ""))

    def _build_jbl_session(self) -> None:
        cfg = self.config
        redactions = cfg.redactions()
        self.jbl = GattSession(
            "JBL",
            [
                cfg.gatttool,
                "-i",
                cfg.adapter,
                "-b",
                cfg.jbl_mac,
                "-t",
                "public",
                "-I",
            ],
            redactions,
        )

    def _build_aura_session(self, address: str, transport: str) -> None:
        cfg = self.config
        redactions = (*cfg.redactions(), address.lower())
        arguments = [cfg.gatttool, "-i", cfg.adapter, "-b", address]
        if transport == "le":
            arguments.extend(["-t", "random", "-I"])
        else:
            arguments.extend(["-p", cfg.aura_psm, "-I"])
        self.aura = GattSession("Aura", arguments, redactions)

    def _close_aura(self) -> None:
        if self.aura is not None:
            self.aura.close()
            self.aura = None

    def _new_aura_resolver(self) -> AuraLeResolver:
        return AuraLeResolver(self.config)

    def _wait_for_aura_retry(
        self, delay: float, deadline: float, attempt: int, total_attempts: int
    ) -> None:
        wait_until = min(deadline, time.monotonic() + delay)
        actual_delay = max(0.0, wait_until - time.monotonic())
        log(
            "Aura FDDF identity was not connectable on scan "
            f"{attempt}/{total_attempts}; retrying after {actual_delay:.1f}s"
        )
        while time.monotonic() < wait_until:
            if self.shutdown_requested:
                raise SessionError("shutdown requested while waiting for Aura")
            time.sleep(min(0.5, max(0.0, wait_until - time.monotonic())))

    def _connect_aura_once(
        self, address: str, transport: str, deadline: float
    ) -> None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise SessionError("Aura connection deadline expired")
        self._close_aura()
        self._build_aura_session(address, transport)
        assert self.aura is not None
        try:
            self.aura.start()
            self.aura.connect(min(self.config.connect_timeout, max(0.1, remaining)))
            # The App enables notifications before sending AA commands. The
            # descriptor handle is identical on the verified BR/EDR and LE DB.
            self.aura.write(
                self.config.aura_cccd_handle,
                "0100",
                self.config.write_timeout,
            )
        except SessionError:
            self._close_aura()
            raise
        self.aura_transport = transport

    def _connect_aura(self, deadline: float) -> None:
        cfg = self.config
        last_error: SessionError | None = None
        le_summary = "not scanned"
        le_hint = ""

        # The canonical cold path is a fresh FDDF advertisement: the LE address
        # rotates, while the payload authenticates it with PID + stable BR/EDR
        # identity.  Try that before spending the scarce window on a classic
        # address that may no longer accept the vendor ATT bearer.
        if cfg.aura_transport in {"auto", "le"}:
            summaries: list[str] = []
            total_attempts = cfg.aura_le_retries + 1
            for attempt in range(1, total_attempts + 1):
                if self.shutdown_requested:
                    raise SessionError("shutdown requested while waiting for Aura")
                if time.monotonic() >= deadline:
                    break
                resolver = self._new_aura_resolver()
                scan_deadline = min(
                    deadline, time.monotonic() + cfg.aura_le_scan_window
                )
                candidate = resolver.resolve(scan_deadline)
                summaries.append(f"scan {attempt}: {resolver.summary()}")
                le_summary = "; ".join(summaries)
                le_hint = resolver.absence_hint()
                if candidate is None:
                    last_error = SessionError(
                        "Aura LE discovery did not resolve an RPA "
                        f"({resolver.summary()}){le_hint}"
                    )
                else:
                    try:
                        self._connect_aura_once(candidate, "le", deadline)
                        return
                    except SessionError as error:
                        last_error = error
                if attempt < total_attempts and time.monotonic() < deadline:
                    self._wait_for_aura_retry(
                        cfg.aura_le_retry_delay,
                        deadline,
                        attempt,
                        total_attempts,
                    )

        if cfg.aura_transport == "le":
            raise SessionError(
                "no verified Aura LE advertisement became connectable "
                f"({le_summary}){le_hint}"
            ) from last_error

        # Preserve the proven stable-address/physical-button route as a bounded
        # fallback. Repeated attempts catch the Aura's brief blue-light window.
        while time.monotonic() < deadline:
            if self.shutdown_requested:
                raise SessionError("shutdown requested while waiting for Aura")
            try:
                self._connect_aura_once(cfg.aura_mac, "bredr", deadline)
                return
            except SessionError as error:
                last_error = error
                time.sleep(0.25)
        raise SessionError(
            "Aura did not become connectable through verified BR/EDR or LE paths"
        ) from last_error

    def connect_sessions(self) -> None:
        normalize_roles = self._previous_state_requires_normalization()
        self._set_state("connecting")
        deadline = time.monotonic() + self.config.aura_connect_window
        # Aura's bearer is the scarce resource, so acquire it before JBL.
        self._connect_aura(deadline)
        self._build_jbl_session()
        assert self.aura is not None and self.jbl is not None
        self.jbl.start()
        self.jbl.connect(self.config.connect_timeout)
        self.jbl.set_mtu(self.config.jbl_mtu, self.config.write_timeout)
        self._set_state("ready")
        if normalize_roles:
            log("normalizing roles after an unclean prior session")
            if not self._best_effort_stop():
                error = "could not normalize roles after an unclean prior session"
                self._set_state("degraded", error)
                raise SessionError(error)
            self._set_state("ready")

    def start_link(self) -> dict[str, object]:
        if self.state == "linked":
            return {"ok": True, "state": self.state, "idempotent": True}
        if self.state not in {"ready"}:
            raise SessionError(f"cannot start from state {self.state}")
        assert self.aura is not None and self.jbl is not None
        self._set_state("starting")
        try:
            self.jbl.write_basic(
                self.config.jbl_handle,
                self.config.enter_frame,
                self.config.write_timeout,
            )
            self.jbl.write(
                self.config.jbl_handle,
                self.config.start_frame,
                self.config.write_timeout,
            )
            if self.config.join_delay > 0:
                time.sleep(self.config.join_delay)
            self.aura.write_aura(
                self.config.aura_handle,
                self.config.aura_on,
                self.config.write_timeout,
                self.config.aura_ack_timeout,
            )
        except SessionError as error:
            self._set_state("degraded", str(error))
            self._best_effort_stop()
            raise
        self._set_state("linked")
        return {
            "ok": True,
            "state": self.state,
            "evidence": "JBL enter response/write ACK plus Aura SetDevInfo success ACK",
        }

    def _best_effort_stop(self) -> bool:
        assert self.aura is not None and self.jbl is not None
        succeeded = True
        try:
            self.aura.write_aura(
                self.config.aura_handle,
                self.config.aura_off,
                self.config.write_timeout,
                self.config.aura_ack_timeout,
            )
        except SessionError as error:
            log(f"best-effort Aura OFF failed: {error}")
            succeeded = False
        try:
            self.jbl.write(
                self.config.jbl_handle,
                self.config.stop_frame,
                self.config.write_timeout,
            )
        except SessionError as error:
            log(f"best-effort JBL STOP failed: {error}")
            succeeded = False
        try:
            self.jbl.write_basic(
                self.config.jbl_handle,
                self.config.exit_frame,
                self.config.write_timeout,
            )
        except SessionError as error:
            log(f"best-effort JBL EXIT failed: {error}")
            succeeded = False
        return succeeded

    def stop_link(self) -> dict[str, object]:
        if self.state == "ready":
            return {"ok": True, "state": self.state, "idempotent": True}
        if self.state not in {"linked", "degraded"}:
            raise SessionError(f"cannot stop from state {self.state}")
        self._set_state("stopping")
        if not self._best_effort_stop():
            error = "one or more persistent stop acknowledgements failed"
            self._set_state("degraded", error)
            raise SessionError(error)
        self._set_state("ready")
        return {
            "ok": True,
            "state": self.state,
            "evidence": "Aura SetDevInfo success ACK plus JBL STOP write/EXIT response",
        }

    def status(self) -> dict[str, object]:
        return {
            "ok": True,
            "state": self.state,
            "pid": os.getpid(),
            "last_error": self.last_error,
            "aura_transport": self.aura_transport,
        }

    def _reply(self, connection: socket.socket, payload: dict[str, object]) -> None:
        connection.sendall((json.dumps(payload, sort_keys=True) + "\n").encode("utf-8"))

    def _handle(self, command: str) -> dict[str, object]:
        if command == "status":
            return self.status()
        if command == "start":
            return self.start_link()
        if command == "stop":
            return self.stop_link()
        if command == "shutdown":
            if self.state != "ready":
                self.stop_link()
            self.running = False
            return {"ok": True, "state": "shutting-down"}
        raise SessionError(f"unknown local command: {command}")

    def _bind_server(self) -> None:
        self._safe_parent(self.socket_path)
        if self.socket_path.is_symlink():
            raise SessionError("refusing symlinked control socket")
        if self.socket_path.exists():
            self.socket_path.unlink()
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(str(self.socket_path))
        os.chmod(self.socket_path, 0o600)
        server.listen(4)
        server.settimeout(0.5)
        self.server = server

    def _request_shutdown(self, _signum: int, _frame: object) -> None:
        self.shutdown_requested = True

    def serve(self) -> int:
        self._acquire_lock()
        signal.signal(signal.SIGTERM, self._request_shutdown)
        signal.signal(signal.SIGINT, self._request_shutdown)
        try:
            self.connect_sessions()
            # Publish the control socket only after both scarce device
            # connections are ready. Clients can never queue a request against
            # a daemon that is still blocked in connect().
            self._bind_server()
            log("persistent control sessions ready")
            assert self.server is not None
            while self.running and not self.shutdown_requested:
                if self.state == "degraded":
                    raise SessionError(
                        self.last_error or "managed session entered degraded state"
                    )
                if not self._control_sessions_connected():
                    raise SessionError(
                        f"control bearer disconnected while state={self.state}"
                    )
                try:
                    connection, _ = self.server.accept()
                except socket.timeout:
                    continue
                with connection:
                    try:
                        data = connection.recv(1024)
                        command = data.decode("utf-8", errors="strict").strip()
                        result = self._handle(command)
                    except (SessionError, UnicodeError, ValueError) as error:
                        result = {"ok": False, "state": self.state, "error": str(error)}
                    try:
                        self._reply(connection, result)
                    except (BrokenPipeError, ConnectionResetError):
                        log("local client disconnected before receiving its reply")
            if self.shutdown_requested and self.state != "ready":
                try:
                    self.stop_link()
                except SessionError as error:
                    log(f"shutdown stop failed: {error}")
            return 0
        except SessionError as error:
            self._set_state("failed", str(error))
            log(f"fatal: {error}")
            return 1
        finally:
            self.cleanup()

    def cleanup(self) -> None:
        if self.server is not None:
            self.server.close()
            self.server = None
        if self.jbl is not None:
            self.jbl.close()
        if self.aura is not None:
            self.aura.close()
        try:
            if self.socket_path.exists() or self.socket_path.is_symlink():
                self.socket_path.unlink()
        except OSError:
            pass
        if self.lock_fd is not None:
            os.close(self.lock_fd)
            self.lock_fd = None
        if self.state not in {"failed", "degraded"}:
            self.state = "offline"
            self.last_error = None
            try:
                self._write_state()
            except OSError:
                pass
        log("control sessions closed")


def client_request(socket_path: Path, command: str, timeout: float) -> int:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(timeout)
    try:
        client.connect(str(socket_path))
        client.sendall((command + "\n").encode("ascii"))
        chunks: list[bytes] = []
        while True:
            chunk = client.recv(4096)
            if not chunk:
                break
            chunks.append(chunk)
            if b"\n" in chunk:
                break
    except (OSError, socket.timeout) as error:
        print(json.dumps({"ok": False, "error": f"session unavailable: {error.strerror or error}"}))
        return 2
    finally:
        client.close()
    raw = b"".join(chunks).split(b"\n", 1)[0]
    try:
        response = json.loads(raw.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError):
        print(json.dumps({"ok": False, "error": "invalid local session response"}))
        return 2
    print(json.dumps(response, sort_keys=True))
    return 0 if response.get("ok") is True else 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="mode", required=True)

    daemon = subparsers.add_parser("daemon")
    daemon.add_argument("--socket", type=Path, required=True)
    daemon.add_argument("--state", type=Path, required=True)
    daemon.add_argument("--lock", type=Path, required=True)

    client = subparsers.add_parser("client")
    client.add_argument("--socket", type=Path, required=True)
    client.add_argument("--timeout", type=float, default=30.0)
    client.add_argument("command", choices=("status", "start", "stop", "shutdown"))
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.mode == "client":
        return client_request(args.socket, args.command, args.timeout)
    try:
        config = Config.from_env()
        supervisor = Supervisor(config, args.socket, args.state, args.lock)
        return supervisor.serve()
    except (OSError, SessionError, ValueError) as error:
        log(f"fatal: {MAC_RE.sub('<bluetooth-address>', str(error))}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
