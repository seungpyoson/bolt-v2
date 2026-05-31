# P3 Adjudication — Provider Bindings + Secret Resolution (PR #480)

HEAD `1f6ee056`. 6 external models reviewed; every finding re-verified vs HEAD bytes.
Verdict: **HARDENING-ONLY** — SSM single secret source holds, secrets zeroized,
provider-leak fence holds for production code. No live-money/secret-critical item.

Anchors use function name + file (line numbers approximate at HEAD; re-locate by name).
**Every fix must preserve or TIGHTEN fail-closed behavior — never loosen a guard.**

## CONFIRMED — actionable

- **F1** `scripts/verify_bolt_v3_provider_leaks.py` — leak-fence regex (`\bnautilus_(?:…)::`)
  misses `use nautilus_x as y` aliasing, bare `use nautilus_x;`, and `extern crate`.
  FIX: strengthen the regex/checks to catch `as`-aliased imports, bare `use`, and
  `extern crate` forms of NT provider crates. Keep the existing registered-crate set.
- **F2** same file — only covers crates discovered from registered bindings; an
  unregistered NT crate (e.g. `nautilus_dydx::`) is unguarded.
  FIX: add a known-NT-crate denylist (or scan Cargo deps) so any `nautilus_*` provider
  crate import in production code is caught, not just the registered ones.
- **F3** `src/bolt_v3_providers/mod.rs` (~586-626) — neutral-named `ClobV2*` materialization
  funcs read as provider-agnostic but are Polymarket-specific, evading the fence intent.
  FIX: rename `ClobV2*` → `PolymarketClobV2*` (or move behind `polymarket::`). **Blast-radius
  check first**: if the rename touches more than mod.rs + polymarket.rs call sites, list the
  call sites and DEFER (report) rather than do a wide rename in this batch.
- **F5** `src/bolt_v3_providers/binance.rs` (`FORBIDDEN_ENV_VARS` ~67-72) — lists 4 vars;
  NT `resolve_credentials` also reads `BINANCE_TESTNET_*`, `BINANCE_FUTURES_TESTNET_*`,
  `BINANCE_DEMO_*` (6 more). Env path is currently dead (Bolt always passes Some), but it's
  a defense-in-depth gap. FIX: add the 6 missing vars to the blocklist.
- **F7** `src/bolt_v3_providers/polymarket.rs` (~486-565) — api_secret is base64-padded before
  the residue scan, so the scan can miss the raw unpadded form. FIX: add the raw form to the
  redaction/residue set, or reject non-canonical secrets.
- **F8** `polymarket.rs` (~198-204) + `binance.rs` (~121-125) — secret fields are `String`,
  only the container is zeroized. FIX: wrap individual secret fields in `Zeroizing<String>`
  (per-field zeroize). Keep redacting Debug impls.
- **F10** `src/bolt_v3_providers/mod.rs` (`ProviderResolvedSecrets::redaction_values` ~43-45) —
  defaults to empty, a footgun for a future provider that forgets to override. FIX: remove the
  default so it's a compile-time required override.
- **F12** `src/bolt_v3_providers/market_data.rs` (~234) — only Kraken calls NT `.validate()`;
  other data providers don't. FIX: apply the NT `.validate()` call uniformly across data
  providers.
- **F16** `polymarket.rs:101` + `binance.rs:66` (`CREDENTIAL_LOG_MODULES`) — NT module paths
  pinned only by a doc comment. FIX: anchor the comment to the pinned NT rev, or add a
  startup/test check that the modules exist. (doc/test hardening)
- **F17** `polymarket.rs` (~486-493) — api_secret lacks base64 pre-store validation (priv_key
  and binance secrets have it). FIX: add base64 validation symmetric with the others.
- **F19** `src/bolt_v3_secrets.rs` (~71-74) — theoretical TOCTOU between env-check and NT
  construction (single-process startup makes it benign). FIX (optional/low): note the
  single-process assumption in a comment, or collapse the check window.

## Optional / doc-only (do if cheap, else report)
- **F4** Cargo.toml — all 8 NT venue crates are unconditional deps (build-graph fence gap).
  Optional: feature-gate or add a dep-scan. Likely DEFER (build-system change).
- **F9** NT-side secret `.clone()` into NT configs as non-zeroized `String` — NT-boundary
  limitation; add an ADR/comment note, do not fight NT.
- **F20** NT Credential Debug leaks api_key upstream — mitigated by the WARN filter; consider
  an upstream issue. No Bolt code change.

## DISPROVEN (do NOT touch)
F6 (Debug leak — wrapper redacts, test asserts), F11 (data-only execution:None — rejected
loud at load), F14 (4 mandatory SSM paths — all required by NT CLOB), F18 (AWS SDK identity
chain — infra identity ≠ trading secret, required by design). F13/F15 are scope-drift to P4
and correct as-is.

## Fix-landing status (current head, 2026-06-01)

Re-verified vs HEAD (verification workflow + personal spot-checks):
- **FIXED-IN-CODE (landed in `dfb4a44e`):** F1, F2, F5, F7, F8, F12, F16, F17, F19 (9 hardening items).
- **DEFERRED-OK:** F4 (build-graph — the leak verifier derives its denylist from Cargo `nautilus-*` deps, so it compensates), F9 (NT-boundary clone — unavoidable NT API constraint), F20 (NT Debug leak — mitigated by the `WARN` filter; upstream concern).
- **F10 — FIXED (this slice):** `ProviderResolvedSecrets::redaction_values` default body removed → required trait method (a missing override is now a COMPILE error); `FakeProviderSecrets` test impl given an explicit empty override. Verified: lib + test compile; 3 redaction tests pass.
- **F3 — DEFERRED (recorded):** the `ClobV2*` materialization types/fns are Polymarket-specific; an explicit ownership comment now sits on the declarations in `src/bolt_v3_providers/mod.rs` (preserves the fence intent). Full rename to `PolymarketClobV2*` is deferred to a dedicated PR (~7-file blast radius: `src/main.rs`, `src/bolt_v3_operator_artifacts.rs` ~85 refs, `polymarket/*` submodules).

**No remaining unaddressed P3 finding.** Ready for external re-review (base `1f6ee056` → current head).

## Coverage cross-check (2026-06-01) — finding in the raw 6-model outputs not captured above

- **DeepSeek P3-F2 / Grok (data-only provider env-var blocklist completeness) — FIXED.** F5 completed and
  NT-anchored the **Binance** `FORBIDDEN_ENV_VARS`, but the six **data-only** blocklists (Bitmex, Bybit,
  Coinbase, Deribit, OKX, Kraken) in `src/bolt_v3_providers/market_data.rs:67-97` had no anchor tying their
  string literals to the env vars the pinned NT adapters actually read — they could silently drift on an NT
  rev bump (the existing `_credential_log_module_paths_exist` anchor covers only the log-module-path
  strings, not the env-var names). Re-verified at HEAD: the lists are complete and correct against NT
  `6e059dc`, but the `check_no_forbidden_credential_env_vars` gate fails closed only on the listed names, so
  a renamed/added NT env var would slip through. Fix: added the CI test
  `forbidden_env_vars_cover_nt_credential_env_vars` (`market_data.rs`, `#[cfg(test)] mod
  forbidden_env_var_anchor_tests`) asserting each blocklist ⊇ the names returned by every adapter's pinned
  `credential_env_vars()` accessor across all environment variants (Bitmex/Bybit/Deribit/Kraken envs,
  Coinbase/OKX argless). Drift now fails CI. The env path is itself only reachable for data-only clients
  passing `None` credentials; the blocklist is the fail-closed guard for that path.
