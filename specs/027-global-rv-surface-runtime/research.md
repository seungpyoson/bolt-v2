# Research: Global RV Surface Runtime

## Decision: Runtime-level RV surface service owns lifecycle

**Decision**: Add a global `RealizedVolSurfaceRuntime` that owns configured surfaces, source bindings, physical subscription keys, observation routing, and latest snapshots by `surface_id`.

**Rationale**: PR #609 moved RV math/policy into a shared engine module but left each binary-oracle taker instance owning its own engine. That prevents shared maker/taker consumption and duplicates state when multiple strategies reference the same surface.

**Alternatives considered**:
- Keep per-strategy engine instances: rejected because it preserves the root failure.
- Add a generic market-state service first: rejected as too broad and not RV-specific enough.
- Publish snapshots only through evidence: rejected because pricing needs runtime access.

## Decision: Taker and maker are snapshot consumers, not RV lifecycle owners

**Decision**: Strategy modules may request/read snapshots by `surface_id`, but may not own source subscriptions, observation fan-out, sampling windows, horizon policy, or aggregation.

**Rationale**: Repo rules require strategies to produce intent only. RV lifecycle is shared market-state infrastructure, not strategy-local signal state.

**Alternatives considered**:
- Leave subscription forwarding in taker: rejected because it still makes taker the RV owner.
- Duplicate forwarding in maker later: rejected because it creates dual lifecycle paths.

## Decision: Multi-venue uses existing configured public data clients first

**Decision**: Production surfaces should add all available, validated public data-client/instrument sources for each asset before adding new provider integrations.

**Rationale**: The repo already has configured public clients such as OKX, Binance, Bybit, Coinbase, Kraken, and others. Using existing clients avoids expanding provider scope while activating multi-source robustness.

**Alternatives considered**:
- Keep OKX-only until math improves: rejected because cross-source robustness remains latent.
- Add new external data vendors first: rejected because existing clients should be exhausted before new dependencies.

## Decision: Multi-horizon RV is the first math upgrade

**Decision**: Compute per-source fixed-grid RV over multiple TOML-owned horizons, then combine through explicit blend/floor/regime policy.

**Rationale**: Multi-horizon RV is auditable, incremental, zero-RV compatible, and directly addresses single-window brittleness. It improves production behavior before heavier estimators.

**Alternatives considered**:
- Realized kernel first: rejected as higher complexity before runtime/global/source foundation is fixed.
- GARCH/ML first: rejected as model-risk heavy and harder to explain in evidence.
- More venues only: rejected because it does not fix single-window math brittleness.

## Decision: Microstructure-noise robustness starts with simple auditable modes

**Decision**: Introduce a configurable quote-midpoint estimator mode such as subsampled RV or multi-scale/pre-averaged RV before full realized-kernel support.

**Rationale**: Quote midpoint feeds can have bid/ask bounce and timing artifacts. Subsampling/pre-averaging is easier to audit and test than full kernel estimators while still reducing false high-frequency volatility.

**Alternatives considered**:
- Coarser grid only: accepted as a baseline option but insufficient as the only noise control.
- Full realized kernel first: deferred until simpler methods prove inadequate.

## Decision: Jump separation, not jump deletion

**Decision**: Add diagnostics that separate continuous RV and jump component, but do not silently remove jumps from final RV unless TOML policy explicitly says so.

**Rationale**: In binary-oracle markets, jumps may be real information. Blind winsorization or truncation can underprice tail movement. Evidence must expose whether RV came from continuous variation, jump contribution, or both.

**Alternatives considered**:
- Winsorize all large returns: rejected because it hides valid market moves.
- Ignore jump diagnostics: rejected because bad ticks and real jumps should not be indistinguishable.

## Decision: Robust aggregation extends upper-quantile/dispersion

**Decision**: Keep upper-quantile as a supported policy but add robust median/MAD or equivalent policies for multi-source surfaces.

**Rationale**: Once sources are multi-venue, one outlier should not silently dominate the aggregate. MAD/median-style diagnostics are intuitive, auditable, and source-count appropriate.

**Alternatives considered**:
- Naive mean: rejected due outlier sensitivity.
- Always max RV: rejected as too conservative and vulnerable to bad high source.
- Always median: rejected as not always conservative enough; policy must be TOML-owned.

## Decision: Forecast layer is optional and measured-vs-forecast evidence stays separate

**Decision**: If added, forecast RV should use simple EWMA/HAR-style methods and evidence must distinguish measured RV from forecast RV.

**Rationale**: Pricing wants future uncertainty, but forecast models introduce model risk. The first implementation should keep measured RV as the authority and make any forecast policy explicit.

**Alternatives considered**:
- GARCH first: rejected due complexity and explainability risk.
- No forecast support ever: rejected because future pricing may need forward-looking uncertainty.

## Decision: External review gate before implementation

**Decision**: Plan/tasks must pass internal adversarial review and relay adversarial reviews from Claude, Gemini, Grok, and GLM before implementation begins.

**Rationale**: The user explicitly required unanimous approval and prior attempts narrowed the scope incorrectly. The review gate prevents another shallow patch cycle.

**Alternatives considered**:
- Implement first, review later: rejected by user instruction and repo review bar.
