# Investigation Report: Monolithic Codebase and Fallback Patterns

## Overview
This document summarizes an investigation into the codebase's monolithic structure and deeply nested conditionals, addressing the concerns raised regarding AI-generated code and fallback patterns. The investigation specifically targeted `src/strategies/binary_oracle_edge_taker/mod.rs` (8,500+ lines) and `src/bolt_v3_submit_admission.rs` (4,400+ lines).

## Root Cause Analysis

### 1. The "Make it Work" Bias (Silent Fallbacks)
AI agents and junior developers often prioritize reaching a compiling and executing state over implementing correct domain logic.
- **The Pattern:** Widespread use of `.unwrap_or()`, `.unwrap_or_default()`, or empty `else` blocks to swallow errors or missing optional values.
- **The Impact:** This prevents immediate crashes but corrupts downstream calculations. For instance, defaulting a missing live reserved liability to `Decimal::ZERO` or returning an `Unclassified` skip reason masks the true origin of a failure. When the system eventually behaves incorrectly, debugging is nearly impossible because the error occurred long before it manifested.

### 2. Iterative Addition of Risk & Gate Checks
The `bolt-v3` system is heavily focused on admission control and risk reservation.
- **The Pattern:** Over time, as new risk gates or capital rules were required, they were sequentially appended as additional `if/else` conditions within master evaluation functions.
- **The Impact:** This organic growth without periodic refactoring leads to deeply stacked conditionals (the "Arrow Anti-Pattern") and monolithic "God Objects" that handle too many distinct responsibilities.

### 3. Fear of Failing Fast
In live trading environments, there is often a hesitation to "crash the system" or eagerly return an `Err`.
- **The Pattern:** Implementing silent fallbacks to keep the process running.
- **The Impact:** In financial systems, silently proceeding with a corrupted or incomplete state is significantly more dangerous than safely aborting the operation. Failing fast and loudly is crucial for maintaining system integrity.

## Action Plan for Refactoring

To transition the codebase to a robust, "fail-fast" architecture, the following multi-phase refactoring strategy is recommended:

### Phase 1: Replace Silent Fallbacks with Explicit Error Propagation
- **Action:** Audit files for `.unwrap_or`, `.unwrap_or_else`, and silent `match` arms.
- **Execution:** Do not blindly replace these with `expect()`. Instead, trace what the missing state represents. If the state is truly required, the function should return a `Result<T, DomainError>` and use the `?` operator to fail fast and explicitly propagate the reason.

### Phase 2: Flatten Nested Conditionals (Guard Clauses)
- **Action:** Invert deeply stacked `if` blocks.
- **Execution:** Utilize early returns and guard clauses. If a pre-condition is not met, return an `Err` immediately. This flattens the structure, leaving the "happy path" un-indented at the end of the function.

### Phase 3: Break Down Monoliths (Domain-Driven Design)
- **Action:** Extract logical blocks from large files into independent modules and structs.
- **Execution:** For example, the evaluation functions in `BoltV3SubmitAdmissionState` should delegate to smaller, isolated policy checkers (e.g., `CapitalAdmissionPolicy::evaluate()`). This improves readability, isolation, and unit testability.

### Phase 4: Enforce "Strict Evidence" Verification
- **Action:** Strictly adhere to the `AGENTS.md` directive regarding Evidence-Driven Verification.
- **Execution:** Ensure that every state transition is proven by tests, rather than relying on production fallbacks or try/catch mechanisms to handle unforeseen scenarios.
