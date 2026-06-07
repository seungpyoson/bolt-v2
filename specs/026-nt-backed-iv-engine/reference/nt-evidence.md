# NautilusTrader Dependency Evidence

**Feature**: `specs/026-nt-backed-iv-engine/`
**Basis**: `Cargo.toml` and `Cargo.lock` on branch head `f994ae15198502aee9227aea5e813d12b8d5bf92` before Phase 1 edits

## Direct Dependency Pin

`Cargo.toml:25-46` pins all direct NautilusTrader crates to:

```text
https://github.com/nautechsystems/nautilus_trader.git
rev = 6e059dcbb59ac1e582132fc431a581936c216c3c
```

Direct NT crates in `Cargo.toml`:

- `nautilus-common`
- `nautilus-core`
- `nautilus-data`
- `nautilus-binance`
- `nautilus-bitmex`
- `nautilus-bybit`
- `nautilus-coinbase`
- `nautilus-deribit`
- `nautilus-hyperliquid`
- `nautilus-kraken`
- `nautilus-live`
- `nautilus-model`
- `nautilus-network`
- `nautilus-okx`
- `nautilus-persistence`
- `nautilus-persistence-macros`
- `nautilus-polymarket`
- `nautilus-portfolio`
- `nautilus-serialization`
- `nautilus-execution`
- `nautilus-system`
- `nautilus-trading`

## Lockfile Evidence

`Cargo.lock:4463-5165` resolves NautilusTrader packages to:

```text
git+https://github.com/nautechsystems/nautilus_trader.git?rev=6e059dcbb59ac1e582132fc431a581936c216c3c#6e059dcbb59ac1e582132fc431a581936c216c3c
```

Locked NT packages observed:

- `nautilus-analysis`
- `nautilus-binance`
- `nautilus-bitmex`
- `nautilus-bybit`
- `nautilus-coinbase`
- `nautilus-common`
- `nautilus-core`
- `nautilus-cryptography`
- `nautilus-data`
- `nautilus-deribit`
- `nautilus-execution`
- `nautilus-hyperliquid`
- `nautilus-kraken`
- `nautilus-live`
- `nautilus-model`
- `nautilus-network`
- `nautilus-okx`
- `nautilus-persistence`
- `nautilus-persistence-macros`
- `nautilus-plugin`
- `nautilus-polymarket`
- `nautilus-portfolio`
- `nautilus-risk`
- `nautilus-serialization`
- `nautilus-system`
- `nautilus-trading`

## Implementation Rules From Evidence

- The IV capability resolver must derive the NT checkout from Cargo metadata and lockfile evidence, not from a handwritten local path.
- The IV capability ledger must classify every discovered IV/options surface from this pinned NT revision before implementation claims coverage.
- Local checkout paths are not recorded as source of truth in this file.
- The old Bolt v1 repository is not used as evidence.

## Deferred To T025-T036

- `cargo metadata --locked` checkout resolution test.
- Seed-family IV/options scan.
- Whole-checkout candidate sweep for IV/options and option-microstructure terms.
- Capability classification fixture and generated ledger loader.
