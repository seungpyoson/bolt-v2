# Tasks: CI Cargo Cache Sharing

**Input**: Design documents from `specs/012-ci-cargo-cache-sharing/`

## Phase 1: Evidence & Research

- [x] T001 Read #333/#366 live issue scope.
- [x] T002 Inspect current `ci.yml` cache topology after merged #250 children.
- [x] T003 Inspect pinned Swatinem/rust-cache v2.9.1 source for `shared-key`, `cache-targets`, `cache-bin`, and `cache-directories` semantics.

## Phase 2: TDD Implementation

- [x] T004 Add failing verifier self-tests for shared registry/git cache and isolated target cache invariants.
- [x] T005 Implement workflow verifier checks for shared registry/git cache.
- [x] T006 Split `ci.yml` cache steps into shared rust-cache and isolated `actions/cache` target caches.

## Phase 3: Verification

- [x] T007 Run verifier self-tests.
- [x] T008 Run workflow verifier.
- [x] T009 Run `just ci-lint-workflow` and `git diff --check`.
- [ ] T010 Push branch, open PR for #366 only, and capture exact-head CI/cache log evidence.
- [ ] T011 Request no-mistakes and external reviews after exact-head CI is green.
