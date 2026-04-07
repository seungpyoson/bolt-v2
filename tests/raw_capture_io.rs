use std::fs;

use bolt_v2::raw_types::{RawHttpResponse, append_jsonl};
use tempfile::tempdir;

#[test]
fn appends_multiple_jsonl_rows() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("responses.jsonl");

    let row = RawHttpResponse {
        endpoint: "/markets".to_string(),
        request_params_json: "{\"slug\":\"election-2028\"}".to_string(),
        received_ts: 1,
        payload_json: "{\"ok\":true}".to_string(),
        source: "polymarket".to_string(),
        parser_version: "v1".to_string(),
        ingest_date: "2026-04-06".to_string(),
    };

    append_jsonl(&path, &row).unwrap();
    append_jsonl(&path, &row).unwrap();

    let text = fs::read_to_string(path).unwrap();
    assert_eq!(text.lines().count(), 2);
}
