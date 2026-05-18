# Quickstart: CI Slow Test Post-Stop Delay

```bash
just test --test venue_contract contract_happy_path_polymarket --no-capture
just test --test venue_contract
just test --test nt_runtime_capture
just test --test lake_batch
just fmt-check
git diff --check
```

Expected representative evidence after the change:

- `Awaiting residual events (0ns)`
- `contract_happy_path_polymarket` completes around 0.04s after compile.
