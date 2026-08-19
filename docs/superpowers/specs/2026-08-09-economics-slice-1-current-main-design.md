# Economics Slice 1 Current-Main Design

## Status

Approved implementation design for #1445. It preserves the accepted architecture from commit `00a5f9e6d7103b52ffcf210e96a3130150352f85` while binding it to authoritative current `main`. Closed PR #1446 and head `23aa59de4` are reference material only and will not be rebased, cherry-picked, or merged.

## Outcome

Replace the current scalar `FeeProvider` authority with one typed economics quote used by execution admission, replay, and evidence. Missing, stale, contradictory, unsupported, or unvalued required economics blocks admission; it never becomes zero. This slice remains offline/shadow and grants no live, deploy, or trading authority.

## Chosen approach

The cutover is developed in one draft PR and becomes mergeable only after every production consumer has moved and `FeeProvider`, provider builders, strategy-owned fee math, and the flat replay fee path are removed. This prevents a merged dual-authority interval.

Two alternatives are rejected:

- Staged production merges would leave both scalar fees and typed economics available.
- Rebasing or cherry-picking #1446 would overwrite accepted runtime configuration and import its stale dependency and source-scanning defects.

## Boundaries

### Neutral core

`crates/economics-core` owns quote requests, signed native effects, quote health, valuation requirements, debit bounds, and edge folding. Its dependency list is restricted to neutral value libraries. It exposes neutral validated identifiers such as `CurrencyId`, never NautilusTrader, Bolt runtime, venue SDK, transport, persistence, filesystem, database, or clock implementation types.

`src/economics/mod.rs` is the sole Bolt-facing facade and re-exports the isolated core. Consumers use `bolt_v2::economics`; they do not import the core crate directly.

### Substrate adapters

`src/integrations/nautilus/economics.rs` converts NautilusTrader instruments and order intent into neutral quote requests and converts approved economics back into admission inputs. Replay owns a separate adapter that maps manifest and catalog facts to the same neutral contracts.

### Venue adapters

Polymarket and Hyperliquid adapters own schedule parsing, authoritative-source validation, venue formulas, rounding, native effect units, and supported capability declarations. No venue formula enters the neutral core.

### Composition

The existing TOML-backed provider registry binds exactly one economics adapter per execution client. `StrategyBuildContext` no longer carries fee authority. Strategies provide intent and gross assumptions only; shared execution/admission requests the economics quote and folds it into sizing and gating.

## Domain rules

- Estimates and actuals are different types; Slice 1 produces estimates only.
- Signed effects retain native currency, attribution, source observation time, and guarantee class.
- Guaranteed conditional costs and validated debit risk bounds may reduce admissible edge.
- Forecast rebates, rewards, or incentives never authorize admission.
- A required effect without a valid valuation route blocks the quote.
- Quote health is typed and exhaustive; unknown economics is not represented by a zero amount.
- Resting orders are re-quoted when economics authority or material quote inputs change.

## Cutover sequence

1. Add the compiler-isolated neutral core and behavior tests.
2. Add NautilusTrader and replay adapters.
3. Add current Polymarket and Hyperliquid venue adapters with fixtures and fail-closed validation.
4. Bind one adapter per execution client through current TOML/runtime composition.
5. Move shared execution/admission, strategy inputs, evidence, and replay to the typed quote.
6. Remove `FeeProvider`, every fee-provider builder, strategy/family fee math, flat replay `fee_bps`, and duplicate quote/report calculations in the same final branch state.

## Verification

- Core behavior tests cover signed effects, health, valuation, debit bounds, edge folding, and invalid input rejection.
- Venue fixtures cover supported formulas, rounding, missing/stale sources, unsupported schedules, and negative maker rates where valid.
- Adapter tests prove NT and replay facts produce the same neutral request and quote semantics.
- Admission tests prove required unknown economics blocks before submit mutation.
- Resting-order tests prove a changed economics quote triggers refresh.
- Compiler dependency direction is enforced by the isolated crate manifest; no source-scanning test is added.
- Exact-head advisory CI supplies Rust compile, Clippy, test, and build evidence. Local compile-heavy Cargo remains unnecessary and, if used, targets the T9 SSD only.

## Scope boundary

Canonical actual ledgers, supplemental venue actuals, lifecycle/transfer economics, reporting closure, and live enablement remain outside Slice 1 and require their existing later-slice ownership. This design introduces no compatibility adapter, fallback, alternate secret source, or parallel production fee route.
