// Codex Cloud auto-review smoke fixture.
//
// This file is not wired into Cargo. It exists only to verify whether Codex
// Cloud auto review responds to a ready pull request with a Rust-looking diff.

fn codex_auto_review_smoke_timeout_ms() -> u64 {
    12345
}
