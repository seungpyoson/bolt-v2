# Data Model: Fee-Provider Binding Decoupling

## FeeProviderResolver

- Input: loaded root config, strategy execution client id, and a borrowed or shared reference to the existing resolved secrets snapshot.
- Output: `Result<Arc<dyn FeeProvider>, FeeProviderResolverError>`.
- Error: registration binding error with strategy instance and archetype context.

Error taxonomy:

- Missing execution client id.
- Unknown provider or execution-client kind.
- Valid execution client with no registered fee-provider capability.
- Provider-specific config parse failure.
- Missing or invalid resolved secret binding for the selected client.
- Provider-specific client construction failure.

Validation:

- Execution client id must exist in loaded clients.
- The loaded client block's TOML-owned `venue` field is the provider dispatch key for the existing provider registry.
- Provider kind must resolve to one registered provider binding.
- Provider-specific config parse, resolved-secret binding, and client construction errors remain concrete binding errors.
- Provider warm failures stay in the existing strategy runtime lifecycle; fee-provider resolution must not introduce registration-time warm calls.
- Resolver-owned errors and concrete binding errors must have secret-safe `Display` and `Debug` output.
- Secret-safety tests must inject sentinel secret values through the resolved secrets snapshot and assert both `Display` and `Debug` output omit the exact sentinel values and raw credential field contents.

## ProviderBinding

- `key`: provider key selected from existing TOML-owned `clients.<id>.venue` and the existing provider registry; no config migration or alternate dispatch source belongs in this issue.
- `build_fee_provider`: generic provider-binding function that receives client key, provider-specific config, and a borrowed or shared resolved secrets snapshot reference, then returns `Result<Arc<dyn FeeProvider>, FeeProviderResolverError>`. The signature must not include Polymarket-specific token, URL, credential, or CLOB types.

Rules:

- Concrete venue logic lives here or below.
- Provider bindings may use NT adapter helpers.
- Future bindings may use a compound provider key if one execution-client kind supports multiple fee models.
- Shared order/admission/runtime core must not import concrete providers.
- Resolver output is per registration call, preserving the existing per-strategy fee-provider lifecycle unless a later approved scope explicitly introduces shared provider caching.
- Concrete bindings may continue resolving provider-specific credential handles from the existing resolved secrets snapshot; generic resolver code must not own raw provider credential fields.
- The generic resolver must not clone raw secret strings out of the snapshot; concrete bindings own provider-specific extraction and redaction.
- Resolver and binding errors must not log, format, or display raw secret material.

## StrategyBuildContext

Existing entity:

- `fee_provider: Arc<dyn FeeProvider>`
- `decision_evidence`
- `submit_admission`

Invariant:

- Strategy code consumes the trait only. It must not know which provider built it.

## SourceFenceInvariant

- Disallow direct `polymarket::build_fee_provider` in all files under `src/bolt_v3_archetypes/`, all strategy modules under `src/strategies/`, `src/bolt_v3_strategy_registration.rs`, `src/bolt_v3_submit_admission.rs`, and `src/bolt_v3_order_intent.rs`.
- Disallow concrete provider imports in shared order/admission/runtime core outside provider registry wiring. The fence must reject at least `bolt_v3_providers::polymarket`, `polymarket::`, and direct `build_fee_provider` usage in prohibited files.
- Allow concrete Polymarket provider construction in `src/bolt_v3_providers/polymarket*`.
- Allow provider registry references in `src/bolt_v3_providers/mod.rs`.
- Source-fence tests must scan source text or an equivalent parsed representation across the prohibited directories/files; merely exercising runtime registration is not enough for the fence.
