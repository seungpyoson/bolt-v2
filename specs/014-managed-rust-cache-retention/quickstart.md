# Quickstart: Managed Rust Cache Retention

Planning commands used:

```sh
df -h ~
du -sh ~/.cache/rust-verification/bolt-v2/target \
  ~/.cache/rust-verification/bolt-v2/target/debug \
  ~/.cache/rust-verification/bolt-v2/target/release \
  ~/.cache/rust-verification/bolt-v2/target/aarch64-unknown-linux-gnu \
  ~/Projects/Claude/bolt-v2/target \
  ~/.cargo/git \
  ~/.cargo/registry
python3 scripts/test_rust_verification.py
python3 scripts/test_rust_verification_cache_retention.py
python3 scripts/test_rust_verification_decoupling.py
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
```

Future intended operator commands:

```sh
python3 scripts/rust_verification.py cache-status --repo . --json
python3 scripts/rust_verification.py cache-prune --repo . --dry-run --json
python3 scripts/rust_verification.py cache-prune --repo . --apply --json
```

Do not run apply mode until dry-run output has been reviewed. Prune output only lists candidates when policy pressure is true: managed cache above `soft_limit_bytes` or filesystem free bytes below `min_free_bytes`.
