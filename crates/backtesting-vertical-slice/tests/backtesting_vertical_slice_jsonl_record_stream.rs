use backtesting_vertical_slice::{
    canonical_trades::RawPayloadContainer,
    jsonl_record_stream::{JsonlStreamLimits, visit_jsonl_records},
};
use flate2::{Compression, write::GzEncoder};
use std::io::{Cursor, Write};

fn limits() -> JsonlStreamLimits {
    JsonlStreamLimits {
        max_decoded_bytes: 128,
        max_members: 1,
        max_member_bytes: 128,
        max_record_bytes: 64,
        max_records: 3,
        member_suffix: None,
    }
}

#[test]
fn plain_jsonl_visits_nonempty_records_in_encounter_order() {
    let input = b"{\"event\":1}\n\n{\"event\":2}\r\n";
    let mut visited = Vec::new();

    let stats = visit_jsonl_records(
        RawPayloadContainer::JsonlText,
        input,
        &limits(),
        |ordinal, record| {
            visited.push((ordinal, record.to_vec()));
            Ok(())
        },
    )
    .expect("visit plain JSONL records");

    assert_eq!(
        visited,
        vec![
            (0, br#"{"event":1}"#.to_vec()),
            (1, br#"{"event":2}"#.to_vec()),
        ]
    );
    assert_eq!(stats.decoded_bytes, input.len() as u64);
    assert_eq!(stats.members, 1);
    assert_eq!(stats.records, 2);
    assert!(stats.peak_record_buffer_bytes <= limits().max_record_bytes + 1);
}

#[test]
fn gzip_jsonl_uses_the_same_bounded_record_contract() {
    let input = b"{\"event\":1}\n{\"event\":2}\n";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).expect("compress fixture");
    let compressed = encoder.finish().expect("finish gzip fixture");
    let mut visited = Vec::new();

    let stats = visit_jsonl_records(
        RawPayloadContainer::JsonlGzip,
        &compressed,
        &limits(),
        |ordinal, record| {
            visited.push((ordinal, record.to_vec()));
            Ok(())
        },
    )
    .expect("visit gzip JSONL records");

    assert_eq!(
        visited,
        vec![
            (0, br#"{"event":1}"#.to_vec()),
            (1, br#"{"event":2}"#.to_vec()),
        ]
    );
    assert_eq!(stats.decoded_bytes, input.len() as u64);
    assert_eq!(stats.members, 1);
    assert_eq!(stats.records, 2);
}

#[test]
fn single_member_zip_uses_the_same_bounded_record_contract() {
    let input = b"{\"event\":1}\n{\"event\":2}\n";
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "events.jsonl",
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .expect("start ZIP member");
    writer.write_all(input).expect("write ZIP member");
    let compressed = writer.finish().expect("finish ZIP fixture").into_inner();
    let mut visited = Vec::new();

    let stats = visit_jsonl_records(
        RawPayloadContainer::SingleJsonlZip,
        &compressed,
        &limits(),
        |ordinal, record| {
            visited.push((ordinal, record.to_vec()));
            Ok(())
        },
    )
    .expect("visit ZIP JSONL records");

    assert_eq!(
        visited,
        vec![
            (0, br#"{"event":1}"#.to_vec()),
            (1, br#"{"event":2}"#.to_vec()),
        ]
    );
    assert_eq!(stats.decoded_bytes, input.len() as u64);
    assert_eq!(stats.members, 1);
    assert_eq!(stats.records, 2);
}

fn tar_gzip_fixture(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, body) in members {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let size = format!("{:011o}\0", body.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[156] = b'0';
        write_tar_checksum(&mut header);
        tar.extend_from_slice(&header);
        tar.extend_from_slice(body);
        let padding = (512 - body.len() % 512) % 512;
        tar.resize(tar.len() + padding, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("compress tar fixture");
    encoder.finish().expect("finish tar fixture")
}

fn write_tar_checksum(header: &mut [u8; 512]) {
    header[148..156].fill(b' ');
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let field = format!("{checksum:06o}");
    header[148..154].copy_from_slice(field.as_bytes());
    header[154] = 0;
    header[155] = b' ';
}

#[test]
fn tar_gzip_visits_matching_members_without_materializing_them() {
    let compressed = tar_gzip_fixture(&[
        ("metadata.txt", b"ignored"),
        ("first.jsonl", b"{\"event\":1}\n"),
        ("second.jsonl", b"{\"event\":2}\n"),
    ]);
    let mut config = limits();
    config.max_decoded_bytes = 4096;
    config.max_members = 3;
    config.member_suffix = Some(".jsonl".to_string());
    let mut visited = Vec::new();

    let stats = visit_jsonl_records(
        RawPayloadContainer::TarGzipJsonl,
        &compressed,
        &config,
        |ordinal, record| {
            visited.push((ordinal, record.to_vec()));
            Ok(())
        },
    )
    .expect("visit tar.gz JSONL records");

    assert_eq!(
        visited,
        vec![
            (0, br#"{"event":1}"#.to_vec()),
            (1, br#"{"event":2}"#.to_vec()),
        ]
    );
    assert_eq!(stats.members, 3, "skipped members still consume the cap");
    assert_eq!(stats.records, 2);
    assert!(stats.decoded_bytes > 2 * 512);
    assert!(stats.peak_record_buffer_bytes <= config.max_record_bytes + 1);
}

#[test]
fn oversized_record_fails_before_callback() {
    let mut config = limits();
    config.max_record_bytes = 4;
    let mut callbacks = 0;

    let error = visit_jsonl_records(
        RawPayloadContainer::JsonlText,
        b"12345\n",
        &config,
        |_, _| {
            callbacks += 1;
            Ok(())
        },
    )
    .expect_err("oversized record must fail");

    assert!(error.to_string().contains("max_record_bytes"), "{error}");
    assert_eq!(callbacks, 0);
}

#[test]
fn exact_limit_record_with_crlf_is_accepted_without_the_line_ending() {
    let mut config = limits();
    config.max_record_bytes = 4;
    let mut visited = Vec::new();

    let stats = visit_jsonl_records(
        RawPayloadContainer::JsonlText,
        b"1234\r\n",
        &config,
        |ordinal, record| {
            visited.push((ordinal, record.to_vec()));
            Ok(())
        },
    )
    .expect("CRLF terminator must not consume the payload-byte limit");

    assert_eq!(visited, vec![(0, b"1234".to_vec())]);
    assert_eq!(stats.records, 1);
    assert_eq!(stats.peak_record_buffer_bytes, config.max_record_bytes + 2);
}

#[test]
fn invalid_utf8_fails_before_callback() {
    let mut callbacks = 0;

    let error = visit_jsonl_records(
        RawPayloadContainer::JsonlText,
        &[0xff, b'\n'],
        &limits(),
        |_, _| {
            callbacks += 1;
            Ok(())
        },
    )
    .expect_err("invalid UTF-8 must fail");

    assert!(error.to_string().contains("UTF-8"), "{error}");
    assert_eq!(callbacks, 0);
}

#[test]
fn cumulative_decoded_byte_limit_includes_blank_lines() {
    let mut config = limits();
    config.max_decoded_bytes = 11;
    let input = b"{\"ok\":1}\n\n\n\n";

    let error = visit_jsonl_records(
        RawPayloadContainer::JsonlText,
        input,
        &config,
        |_, _| Ok(()),
    )
    .expect_err("blank lines still consume the decoded-byte cap");

    assert!(error.to_string().contains("max_decoded_bytes"), "{error}");
}

#[test]
fn record_count_limit_applies_across_nonempty_records() {
    let mut config = limits();
    config.max_records = 1;

    let error = visit_jsonl_records(
        RawPayloadContainer::JsonlText,
        b"{\"event\":1}\n{\"event\":2}\n",
        &config,
        |_, _| Ok(()),
    )
    .expect_err("second record exceeds the configured count");

    assert!(error.to_string().contains("max_records"), "{error}");
}

#[test]
fn single_stream_member_limit_applies_to_plain_and_gzip_jsonl() {
    let input = b"{\"event\":1}\n";
    let mut config = limits();
    config.max_member_bytes = (input.len() - 1) as u64;

    for (container, bytes) in [
        (RawPayloadContainer::JsonlText, input.to_vec()),
        (RawPayloadContainer::JsonlGzip, {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(input).expect("compress fixture");
            encoder.finish().expect("finish gzip fixture")
        }),
    ] {
        let error = visit_jsonl_records(container, &bytes, &config, |_, _| Ok(()))
            .expect_err("single JSONL stream must obey max_member_bytes");

        assert!(error.to_string().contains("max_member_bytes"), "{error}");
    }
}

#[test]
fn truncated_gzip_fails_loud() {
    let input = b"{\"event\":1}\n{\"event\":2}\n";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).expect("compress fixture");
    let mut compressed = encoder.finish().expect("finish gzip fixture");
    compressed.truncate(compressed.len() / 2);

    let error = visit_jsonl_records(
        RawPayloadContainer::JsonlGzip,
        &compressed,
        &limits(),
        |_, _| Ok(()),
    )
    .expect_err("truncated gzip must fail");

    assert!(!error.to_string().is_empty());
}

#[test]
fn tar_member_count_includes_nonmatching_members() {
    let compressed = tar_gzip_fixture(&[
        ("first.meta", b"ignored"),
        ("second.meta", b"ignored"),
        ("events.jsonl", b"{\"event\":1}\n"),
    ]);
    let mut config = limits();
    config.max_decoded_bytes = 4096;
    config.max_members = 2;
    config.member_suffix = Some(".jsonl".to_string());

    let error = visit_jsonl_records(
        RawPayloadContainer::TarGzipJsonl,
        &compressed,
        &config,
        |_, _| Ok(()),
    )
    .expect_err("third archive member exceeds the cap even when earlier members do not match");

    assert!(error.to_string().contains("max_members"), "{error}");
}

#[test]
fn tar_member_byte_limit_includes_nonmatching_members() {
    let compressed = tar_gzip_fixture(&[
        ("large.meta", b"larger-than-cap"),
        ("events.jsonl", b"{\"event\":1}\n"),
    ]);
    let mut config = limits();
    config.max_decoded_bytes = 3072;
    config.max_members = 2;
    config.max_member_bytes = 8;
    config.member_suffix = Some(".jsonl".to_string());

    let error = visit_jsonl_records(
        RawPayloadContainer::TarGzipJsonl,
        &compressed,
        &config,
        |_, _| Ok(()),
    )
    .expect_err("nonmatching member still consumes the per-member byte cap");

    assert!(error.to_string().contains("max_member_bytes"), "{error}");
}

#[test]
fn generated_large_tar_keeps_the_record_buffer_bounded() {
    let record = b"{\"event\":1}\n";
    let repetitions = 100_000usize;
    let mut body = Vec::with_capacity(record.len() * repetitions);
    for _ in 0..repetitions {
        body.extend_from_slice(record);
    }
    let compressed = tar_gzip_fixture(&[("events.jsonl", body.as_slice())]);
    let mut config = limits();
    config.max_decoded_bytes = body.len() as u64 + 2048;
    config.max_members = 1;
    config.max_member_bytes = body.len() as u64;
    config.max_records = repetitions as u64;
    config.member_suffix = Some(".jsonl".to_string());
    let mut callbacks = 0u64;

    let stats = visit_jsonl_records(
        RawPayloadContainer::TarGzipJsonl,
        &compressed,
        &config,
        |_, _| {
            callbacks += 1;
            Ok(())
        },
    )
    .expect("scan generated large tar");

    assert_eq!(callbacks, repetitions as u64);
    assert_eq!(stats.records, repetitions as u64);
    assert!(stats.peak_record_buffer_bytes <= config.max_record_bytes + 1);
}

#[test]
fn tar_decoded_byte_limit_includes_headers_and_skipped_payloads() {
    let compressed = tar_gzip_fixture(&[
        ("ignored.meta", &[b'x'; 700]),
        ("events.jsonl", b"{\"event\":1}\n"),
    ]);
    let mut config = limits();
    config.max_decoded_bytes = 512;
    config.max_members = 2;
    config.max_member_bytes = 1024;
    config.member_suffix = Some(".jsonl".to_string());

    let error = visit_jsonl_records(
        RawPayloadContainer::TarGzipJsonl,
        &compressed,
        &config,
        |_, _| Ok(()),
    )
    .expect_err("skipped tar bytes must consume the cumulative decoded cap");

    assert!(error.to_string().contains("max_decoded_bytes"), "{error}");
}

#[test]
fn zip_with_multiple_members_is_rejected() {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, record) in [
        ("first.jsonl", b"{\"event\":1}\n".as_slice()),
        ("second.jsonl", b"{\"event\":2}\n".as_slice()),
    ] {
        writer
            .start_file(name, zip::write::FileOptions::default())
            .expect("start ZIP member");
        writer.write_all(record).expect("write ZIP member");
    }
    let compressed = writer.finish().expect("finish ZIP fixture").into_inner();

    let error = visit_jsonl_records(
        RawPayloadContainer::SingleJsonlZip,
        &compressed,
        &limits(),
        |_, _| Ok(()),
    )
    .expect_err("single_jsonl_zip must reject a second member");

    assert!(error.to_string().contains("exactly one"), "{error}");
}
