# Investigation Report: Managing Fallbacks, Deep Nesting, and Monolithic Functions

## Executive Summary
This report investigates the prevalence of stacked conditionals, deep nesting, and monolithic functions within the `bolt-v2` codebase, as highlighted by senior engineering feedback. We found that the extensive use of fallbacks (e.g., `unwrap_or`, `unwrap_or_default`) and nested `if let` blocks is a direct result of AI coding agents prioritizing "making it work" and circumventing strict type/error checks, rather than failing closed as per standard robust engineering practices.

## Findings
A thorough scan of the codebase reveals the following metrics:
- **`unwrap_or` and `unwrap_or_default` usages:** ~95 instances total. These are frequently used to provide silent default values (e.g., `0` or `false`) when data is unexpectedly missing, masking downstream errors and making debugging difficult.
- **Deep Nesting:** Extensive use of `if let` statements (over 500 occurrences) often leads to deeply nested code (up to 29 levels deep in `src/bolt_v3_live_node.rs` and `src/bolt_v3_validate.rs`).
- **Monolithic Functions:** Several functions exceed reasonable length limits, obscuring logic and violating single-responsibility principles. For instance, `entry_evaluation_at()` in `src/strategies/binary_oracle_edge_taker/mod.rs` is 374 lines long.

### Why does this happen?
As the senior engineer pointed out, AI coding agents (and sometimes junior developers) are highly optimized for delivering a "working" output quickly. To achieve this, they often deploy "fallbacks" - silently swallowing errors by returning a default value or trapping them inside an empty `else` block of an `if let`. While this allows the software to compile and run initially without immediate crashes, it creates fragile systems that fail unexpectedly under edge cases or high load, masking the root cause of the failure.

## Refactoring Strategy & Guidelines
To remedy this and align with the `AGENTS.md` directive of "no technical debt" and "fail-closed" behavior, the following strategies must be adopted going forward:

1. **Eliminate Silent Fallbacks:**
   Replace `unwrap_or` and `unwrap_or_default` with strict error propagation using `Result<T, E>` and the `?` operator. If a value is genuinely expected to be missing in normal operation, use explicit `match` blocks to handle the `None` case deliberately rather than defaulting.

2. **Fail Closed:**
   If a required value is missing (e.g., a critical capital admission decision), the system must explicitly reject or fail the operation rather than falling back to a default "safe" value.

3. **Flatten Nested Conditionals:**
   Refactor deeply nested `if let` blocks using early returns and guard clauses (`let Some(x) = y else { return ... }`). This improves readability and exposes unhandled failure paths.

4. **Decompose Monolithic Functions:**
   Large functions over 100 lines (e.g., `entry_evaluation_at`) should be broken down into smaller, composable helper functions. Each function should have a single, clear responsibility.

## Implementation Example
In this PR, we have applied these principles to `src/bolt_v3_submit_admission.rs` as a proof-of-concept, replacing silent fallbacks like `.unwrap_or(0)` and `.unwrap_or(BoltV3CapitalAdmissionRejectReason::Rejected)` with explicit error handling and missing-value state representation.
