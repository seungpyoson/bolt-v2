# Research: #466 Disk-Governance Verifier Decomposition

## Decision: Use the #466 scope ledger as the completion source of truth

**Rationale**: Issue #466 is explicitly a continuation after #465, not a single helper slice. The ledger records every remaining item and prevents a partial PR from being treated as issue completion.

**Alternatives considered**: Relying on PR bodies alone was rejected because PR #465 already proved a valid slice can coexist with unresolved broader scope.

## Decision: Keep divergent helper families local unless a focused slice proves equivalence

**Rationale**: Current-main evidence shows tokenization, shell substitution, renamed tool detection, wrapper handling, and full target-routing policy have different runtime/static boundaries. Extracting them without proof would risk behavior drift.

**Alternatives considered**: Broad extraction was rejected because prior #454/#464 review evidence identified semantic divergence across those candidates.

## Decision: Treat `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT` as the first likely cleanup candidate

**Rationale**: Current tests already assert static/shared parity for this constant, while static `CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT` intentionally differs. Importing the shared with-argument constant may remove a drift source without changing static option consumption semantics.

**Alternatives considered**: Keeping only the parity assertion remains a fallback if external review finds import coupling risky.

## Decision: Mechanical file splitting is optional, evidence-gated work

**Rationale**: The verifier files are large, but a split that changes import order or hides behavior change would increase review risk. A split is justified only when a concern boundary is explicit and the same tests prove preservation.

**Alternatives considered**: Splitting files for size alone was rejected.

## Decision: External plan review blocks implementation

**Rationale**: The prompt requires Claude, Gemini, Grok, GLM, DeepSeek, and Kimi plan/task/evidence review before code. Missing or failed reviewer output is not approval unless the operator explicitly waives that slot.

**Alternatives considered**: Proceeding with local-only confidence was rejected by the prompt and repo review bar.
