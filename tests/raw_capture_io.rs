use std::fs;

use bolt_v2::raw_types::{JsonlAppender, RawHttpResponse, append_jsonl};
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

#[test]
fn jsonl_appender_reuses_same_path_for_multiple_rows() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("responses.jsonl");
    let mut appender = JsonlAppender::new();

    let row = RawHttpResponse {
        endpoint: "/markets".to_string(),
        request_params_json: "{\"slug\":\"election-2028\"}".to_string(),
        received_ts: 1,
        payload_json: "{\"ok\":true}".to_string(),
        source: "polymarket".to_string(),
        parser_version: "v1".to_string(),
        ingest_date: "2026-04-06".to_string(),
    };

    appender.append(&path, &row).unwrap();
    appender.append(&path, &row).unwrap();
    appender.close().unwrap();

    let text = fs::read_to_string(path).unwrap();
    assert_eq!(text.lines().count(), 2);
}

#[test]
fn jsonl_appender_reopens_when_target_path_changes() {
    let dir = tempdir().unwrap();
    let day_one = dir.path().join("2026-04-06").join("responses.jsonl");
    let day_two = dir.path().join("2026-04-07").join("responses.jsonl");
    let mut appender = JsonlAppender::new();

    let row = RawHttpResponse {
        endpoint: "/markets".to_string(),
        request_params_json: "{\"slug\":\"election-2028\"}".to_string(),
        received_ts: 1,
        payload_json: "{\"ok\":true}".to_string(),
        source: "polymarket".to_string(),
        parser_version: "v1".to_string(),
        ingest_date: "2026-04-06".to_string(),
    };

    appender.append(&day_one, &row).unwrap();
    appender.append(&day_two, &row).unwrap();
    appender.close().unwrap();

    let first_text = fs::read_to_string(day_one).unwrap();
    let second_text = fs::read_to_string(day_two).unwrap();
    assert_eq!(first_text.lines().count(), 1);
    assert_eq!(second_text.lines().count(), 1);
}
