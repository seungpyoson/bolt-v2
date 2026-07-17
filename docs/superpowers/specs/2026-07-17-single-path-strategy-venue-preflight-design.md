# Single-Path Strategy Venue Preflight Design

## Scope

PR #1442 must establish every strategy's configured execution venue before any
strategy registration callback can mutate `LiveNode`. This strengthens issue
#1431 B1/B2 without claiming the remaining B3 or D scope.

## Configuration Authority

The venue identity has one source:

`strategy.execution_client_id -> root.clients[execution_client_id].venue`

Production code must not contain a venue, client ID, account, currency, default,
or alternate routing literal. Missing configuration is an error. Runtime health
may block execution at the configured venue, but it must never select another
venue.

## Design

`StrategyRegistrationContext::new` returns `Result` and resolves the configured
execution client and venue for every strategy through one shared helper. The
context stores the venue privately and constructs its fee provider during the
same preflight from that exact client and venue. If the strategy declares the
settlement capability, its settlement account and currency are derived from the
same stored venue.

The registration dispatcher constructs and validates every context into a
prepared collection before invoking any binding callback. Context construction
uses inert binding metadata and performs no `LiveNode` mutation. Once preflight
succeeds, callbacks consume the prepared contexts.

`assemble_strategy_build_context` reads the private stored venue and fee
provider. The prepared context does not expose the resolved-secret registry to
strategy callbacks. Raw strategy-config builders do not repeat execution-client
existence checks. The late `execution_venue_for_context` lookup, fee-provider
venue read, and settlement/non-settlement venue selection branch are removed.
There is no default, retry, conditional fallback, or second execution-venue
resolution path.
Configured signal and reference data clients remain separate routes and are not
execution-venue fallbacks.

## Risk Controls And Evidence

| Risk | Structural control | Required evidence |
| --- | --- | --- |
| A later invalid strategy leaves earlier registrations behind | Collect all validated contexts before callbacks | Invalid second settlement and non-settlement strategies produce zero callbacks and zero registrations |
| A strategy or fee provider resolves a different venue later | One client-map lookup, one private stored venue, and one preflight-built fee provider | Structural fence pins the single execution-client caller and rejects callback rereads |
| Settlement identity uses a different venue | Settlement resolution receives the already-resolved venue | Venue/account/currency failure tests and exact-argument provenance checks |
| Preflight executes user binding behavior | Shared fee-provider construction is allowed; binding kind remains inert metadata and the registration function is not called during preparation | Panic/counting callback regression remains unreachable on preflight failure |
| Missing configuration is silently replaced | All missing venue/account/currency inputs return `Binding` errors | Fail-closed tests cover each missing identity and assert no unwind |
| Runtime outage changes routing | Runtime health/admission may reject, never reroute | Direct inspection and source-fence evidence show no alternate venue selection |
| Test-only or macro-generated code preserves a decoy shape | Final fence scans cfg-free production attribution and exact dataflow, with no legacy accepted shape | Adversarial fixtures cover `cfg(test)`, macro invocation, shadowing, and alternate construction |

## Verification

The implementation follows evidence-driven test-first verification. Add the
non-settlement partial-registration regression before production changes and
confirm it fails for the missing preflight guarantee. After implementation, run
the governed local formatting and static source-fence checks, internal
adversarial review, and exact-head remote Rust verification. Human approval and
native review-thread resolution remain mandatory before queueing.
