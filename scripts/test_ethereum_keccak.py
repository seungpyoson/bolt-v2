#!/usr/bin/env python3
from __future__ import annotations

import unittest

from ethereum_keccak import keccak_256


class EthereumKeccakTests(unittest.TestCase):
    def test_empty_input_matches_ethereum_known_answer(self) -> None:
        self.assertEqual(
            keccak_256(b"").hex(),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
        )

    def test_redeem_positions_selector_is_derived(self) -> None:
        signature = b"redeemPositions(address,bytes32,bytes32,uint256[])"
        self.assertEqual(keccak_256(signature)[:4].hex(), "01b7037c")


if __name__ == "__main__":
    unittest.main()
