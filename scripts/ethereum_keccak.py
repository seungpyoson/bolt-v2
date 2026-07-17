#!/usr/bin/env python3
"""Dependency-free Ethereum Keccak-256 used for ABI selector derivation."""

from __future__ import annotations


_MASK_64 = (1 << 64) - 1
_RATE_BYTES = 136
_ROTATIONS = (
    0, 1, 62, 28, 27,
    36, 44, 6, 55, 20,
    3, 10, 43, 25, 39,
    41, 45, 15, 21, 8,
    18, 2, 61, 56, 14,
)
_ROUND_CONSTANTS = (
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808A,
    0x8000000080008000,
    0x000000000000808B,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008A,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000A,
    0x000000008000808B,
    0x800000000000008B,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800A,
    0x800000008000000A,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
)


def _rotate_left(value: int, shift: int) -> int:
    return ((value << shift) | (value >> ((64 - shift) % 64))) & _MASK_64


def _keccak_f1600(state: list[int]) -> None:
    for round_constant in _ROUND_CONSTANTS:
        columns = [
            state[x]
            ^ state[x + 5]
            ^ state[x + 10]
            ^ state[x + 15]
            ^ state[x + 20]
            for x in range(5)
        ]
        theta = [
            columns[(x - 1) % 5] ^ _rotate_left(columns[(x + 1) % 5], 1)
            for x in range(5)
        ]
        for y in range(5):
            for x in range(5):
                state[x + 5 * y] ^= theta[x]

        rotated = [0] * 25
        for y in range(5):
            for x in range(5):
                rotated[y + 5 * ((2 * x + 3 * y) % 5)] = _rotate_left(
                    state[x + 5 * y], _ROTATIONS[x + 5 * y]
                )

        for y in range(5):
            row = rotated[5 * y : 5 * y + 5]
            for x in range(5):
                state[x + 5 * y] = row[x] ^ ((~row[(x + 1) % 5]) & row[(x + 2) % 5])
                state[x + 5 * y] &= _MASK_64
        state[0] ^= round_constant


def keccak_256(message: bytes) -> bytes:
    """Return Ethereum Keccak-256, which uses the legacy 0x01 domain suffix."""
    padding_length = _RATE_BYTES - (len(message) % _RATE_BYTES)
    padded = bytearray(message)
    padded.extend(b"\x00" * padding_length)
    padded[len(message)] = 0x01
    padded[-1] |= 0x80

    state = [0] * 25
    for offset in range(0, len(padded), _RATE_BYTES):
        block = padded[offset : offset + _RATE_BYTES]
        for lane in range(_RATE_BYTES // 8):
            start = lane * 8
            state[lane] ^= int.from_bytes(block[start : start + 8], "little")
        _keccak_f1600(state)

    return b"".join(state[lane].to_bytes(8, "little") for lane in range(4))
