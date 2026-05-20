# Quickstart: Maker Order Scope Verification

Run from repo root or worktree root.

## Static Evidence

```bash
rg -n "nautilus-polymarket|rev = \"7c2aafb" Cargo.toml Cargo.lock
rg -n "TimeInForce::Gtc|TimeInForce::Gtd|post_only|postOnly|expire_time" \
  /Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/7c2aafb/crates/adapters/polymarket/src
```

## Focused Tests

```bash
CARGO_TARGET_DIR=/tmp/bolt-v2-maker-commit-target /Users/spson/.cargo/bin/cargo test bolt_v3_archetype_accepts_post_only_gtc -- --nocapture
CARGO_TARGET_DIR=/tmp/bolt-v2-maker-commit-target /Users/spson/.cargo/bin/cargo test binary_oracle_runtime_mapping_preserves_post_only_gtc -- --nocapture
CARGO_TARGET_DIR=/tmp/bolt-v2-maker-commit-target /Users/spson/.cargo/bin/cargo test polymarket_post_order_params_declares_camel_case_is_post_only_flag -- --nocapture
CARGO_TARGET_DIR=/tmp/bolt-v2-maker-commit-target /Users/spson/.cargo/bin/cargo test post_only -- --nocapture
```

## Quality Gates

```bash
/Users/spson/.cargo/bin/cargo fmt -- --check
git diff --check
CARGO_TARGET_DIR=/tmp/bolt-v2-maker-commit-target /Users/spson/.cargo/bin/cargo test
```

## Proof Boundary

These checks prove static source behavior and tests. They do not prove live submit, venue accept, no-submit readiness, or tiny-canary readiness.
