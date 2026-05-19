# Requirements Checklist: Managed Rust Cache Retention

**Purpose**: Unit-test requirement quality before implementation.
**Created**: 2026-05-19
**Feature**: `specs/014-managed-rust-cache-retention/spec.md`

## Requirement Completeness

- [x] CHK001 Is the PR target exactly `bolt-v2#286`?
- [x] CHK002 Are `#374` shell bypass and temp/worktree lifecycle explicitly out of scope?
- [x] CHK003 Are status, dry-run, active-process refusal, and apply mode all specified?
- [x] CHK004 Is "do not delete whole target root in any mode" explicit?
- [x] CHK004A Are retention thresholds defined in TOML policy or explicit CLI flags rather than hidden code constants?
- [x] CHK004B Is `tmp` limited to `<managed-target-root>/tmp`, with `/private/tmp/bolt-v2-*` left to `#374`?
- [x] CHK004C Is `other` class behavior explicit?

## Requirement Clarity

- [x] CHK005 Are reported fields measurable: bytes, paths, mtimes, free disk?
- [x] CHK006 Is dry-run default behavior unambiguous?
- [x] CHK007 Is apply mode explicitly gated by a flag?
- [x] CHK008 Are profile/target classes named: debug, release, cross-target, managed-root tmp, other?
- [x] CHK008A Are active-process patterns policy configured?
- [x] CHK008B Is active-process relatedness based on configured process pattern plus repo/managed-target evidence, with fail-closed behavior when visibility is insufficient?
- [x] CHK008C Is cross-target classification defined?
- [x] CHK008D Is symlink/special-file scanner behavior defined?

## Requirement Consistency

- [x] CHK009 Does spec keep repo-local `scripts/rust_verification.py` as owner after #398?
- [x] CHK010 Does spec avoid old `.claude/rust-verification.toml` and global owner paths?
- [x] CHK011 Does plan preserve no runtime/trading/Cargo source changes?
- [x] CHK011A Is `ci/rust-verification.toml` the only persistent policy location?

## Acceptance Criteria Quality

- [x] CHK012 Can tests run without local Cargo?
- [x] CHK013 Does dry-run prove reclaimable bytes without deleting files?
- [x] CHK014 Does active-process refusal have a testable seam?

## Dependencies & Assumptions

- [x] CHK015 Is S3 rejected as active Cargo target replacement?
- [x] CHK016 Is local Cargo still retained for dirty/unpushed/no-PR work?
- [x] CHK017 Is `#123` identified as the parent disk-pressure epic?
- [x] CHK018 Is detailed evidence assigned to `#286` and summary/link assigned to `#123`?
- [x] CHK019 Is no-mistakes limited to a final pre-PR check, not daemon implementation?
