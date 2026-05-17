# Slow Test Post-Stop Evidence - 2026-05-17

Issue: #357

## Before

Command:

```bash
just test --test venue_contract contract_happy_path_polymarket --no-capture
```

Evidence:

- Test result: pass
- Nextest runtime: `PASS [  10.056s]`
- Rust test runtime: `finished in 10.04s`
- NT log: `Awaiting residual events (10s)`

## After

Command:

```bash
just test --test venue_contract contract_happy_path_polymarket --no-capture
```

Evidence:

- Test result: pass
- Nextest runtime: `PASS [   0.057s]`
- Rust test runtime: `finished in 0.04s`
- NT log: `Awaiting residual events (0ns)`

## Scope

This evidence proves the representative NT post-stop delay removal. It does not move any test out of PR CI and does not change production runtime defaults.

## Targeted Verification

```bash
just test --test venue_contract --test nt_runtime_capture --test lake_batch
```

Result: `58 tests run: 58 passed, 0 skipped`, nextest summary `1.576s`.

```bash
just fmt-check
git diff --check
```

Result: pass.
