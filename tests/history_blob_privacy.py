#!/usr/bin/env python3
"""Scan every reachable, unique Git blob without printing names or contents."""

from __future__ import annotations

import re
import subprocess
import sys
from collections.abc import Iterable


CHUNK_SIZE = 1024 * 1024
OVERLAP_SIZE = 512


def _run_git(args: list[str], *, input_bytes: bytes | None = None) -> bytes:
    completed = subprocess.run(
        ["git", *args],
        input=input_bytes,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError("git command failed")
    return completed.stdout


def _unique_reachable_blobs() -> list[bytes]:
    raw_objects = _run_git(["rev-list", "--objects", "--all", "--no-object-names"])
    object_ids = list(dict.fromkeys(raw_objects.splitlines()))
    if not object_ids:
        return []

    checked = _run_git(
        ["cat-file", "--batch-check=%(objectname) %(objecttype)"],
        input_bytes=b"\n".join(object_ids) + b"\n",
    )
    blobs: list[bytes] = []
    for line in checked.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[1] == b"blob":
            blobs.append(fields[0])
    return blobs


def _patterns() -> tuple[re.Pattern[bytes], ...]:
    begin_word = b"BE" + b"GIN"
    private_word = b"PRI" + b"VATE"
    certificate_word = b"CERT" + b"IFICATE"

    boundary = re.compile(
        re.escape(begin_word)
        + rb" (((RSA|EC|DSA|OPENSSH|ENCRYPTED) )?"
        + re.escape(private_word)
        + rb" KEY|PGP "
        + re.escape(private_word)
        + rb" KEY BLOCK|((X509|TRUSTED) )?"
        + re.escape(certificate_word)
        + rb"|"
        + re.escape(certificate_word)
        + rb" REQUEST|(PKCS7|CMS))"
    )
    token = re.compile(
        rb"(gh"
        + rb"[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|"
        + rb"AKIA[0-9A-Z]{16}|AI"
        + rb"za[0-9A-Za-z_-]{35}|xox[baprs]-[A-Za-z0-9-]{10,}|"
        + rb"glpat-[A-Za-z0-9_-]{20,}|sk_(live|test)_[A-Za-z0-9]{20,}|"
        + rb"sk-(proj-)?[A-Za-z0-9_-]{20,}|e"
        + rb"yJ[A-Za-z0-9_-]{8,}[.][A-Za-z0-9_-]{8,}[.]"
        + rb"[A-Za-z0-9_-]{8,})"
    )
    der_body = re.compile(
        rb"(^|[^A-Za-z0-9+/])M[A-Za-z0-9+/]{39,}={0,2}"
        + rb"([^A-Za-z0-9+/=]|$)"
    )
    openssh_body = re.compile(rb"b3BlbnNzaC1rZXktdjE[A-Za-z0-9+/=]{20,}")
    return boundary, token, der_body, openssh_body


def _contains_nonplaceholder_pin(payload: bytes) -> bool:
    pin_name = b"JBL_LOCAL_API_TLS_SHA" + b"256"
    pin_pattern = re.compile(
        re.escape(pin_name) + rb"[ \t]*=[ \t]*([0-9A-Fa-f]{64})"
    )
    return any(set(match.group(1)) != {ord("0")} for match in pin_pattern.finditer(payload))


def _read_exact(stream: object, size: int) -> bytes:
    chunks: list[bytes] = []
    remaining = size
    while remaining:
        chunk = stream.read(remaining)  # type: ignore[attr-defined]
        if not chunk:
            raise RuntimeError("truncated cat-file output")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _scan_blob_stream(
    stream: object,
    size: int,
    patterns: Iterable[re.Pattern[bytes]],
) -> tuple[bool, bool]:
    binary = False
    sensitive = False
    remaining = size
    tail = b""
    while remaining:
        chunk = _read_exact(stream, min(CHUNK_SIZE, remaining))
        remaining -= len(chunk)
        binary = binary or b"\0" in chunk
        window = tail + chunk
        sensitive = sensitive or any(pattern.search(window) for pattern in patterns)
        sensitive = sensitive or _contains_nonplaceholder_pin(window)
        tail = window[-OVERLAP_SIZE:]
    return binary, sensitive


def scan_history() -> tuple[int, int, int]:
    blob_ids = _unique_reachable_blobs()
    if not blob_ids:
        return 0, 0, 0

    patterns = _patterns()
    process = subprocess.Popen(
        ["git", "cat-file", "--batch"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if process.stdin is None or process.stdout is None:
        raise RuntimeError("could not open cat-file pipes")

    binary_blobs = 0
    sensitive_blobs = 0
    try:
        for object_id in blob_ids:
            process.stdin.write(object_id + b"\n")
            process.stdin.flush()
            header = process.stdout.readline()
            fields = header.split()
            if len(fields) != 3 or fields[1] != b"blob":
                raise RuntimeError("unexpected cat-file header")
            size = int(fields[2])
            binary, sensitive = _scan_blob_stream(process.stdout, size, patterns)
            if _read_exact(process.stdout, 1) != b"\n":
                raise RuntimeError("invalid cat-file delimiter")
            binary_blobs += int(binary)
            sensitive_blobs += int(sensitive)
    finally:
        process.stdin.close()
        process.stdout.close()
        process.wait()

    if process.returncode != 0:
        raise RuntimeError("cat-file failed")
    return len(blob_ids), binary_blobs, sensitive_blobs


def main() -> int:
    try:
        total, binary_blobs, sensitive_blobs = scan_history()
    except (OSError, RuntimeError, ValueError):
        print("history blob privacy: scan failed; details redacted", file=sys.stderr)
        return 2

    failed = False
    if binary_blobs:
        print(
            "history blob privacy: binary blob found "
            f"(blobs={binary_blobs}; details redacted)",
            file=sys.stderr,
        )
        failed = True
    if sensitive_blobs:
        print(
            "history blob privacy: credential-shaped blob found "
            f"(blobs={sensitive_blobs}; details redacted)",
            file=sys.stderr,
        )
        failed = True
    if failed:
        return 1

    print(f"history blob privacy: PASS scanned_blobs={total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
