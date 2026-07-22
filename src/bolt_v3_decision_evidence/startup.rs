use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use super::{
    LoadedBoltV3Config, decode::read_registered_stream, machine_decision_evidence_path,
    retired_decision_evidence_paths,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentMachineStreamPreflight {
    pub records: usize,
    pub bytes: u64,
}

pub fn preflight_current_machine_stream(
    loaded: &LoadedBoltV3Config,
) -> Result<CurrentMachineStreamPreflight> {
    ensure_retired_paths_absent(&retired_decision_evidence_paths(loaded)?)?;

    let machine_path = machine_decision_evidence_path(loaded)?;
    let max_bytes = loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes
        .context("current machine-stream preflight requires recovery_evidence_max_bytes")?;
    scan_current_machine_stream(&machine_path, max_bytes)
}

fn ensure_retired_paths_absent(retired_paths: &[PathBuf]) -> Result<()> {
    for retired_path in retired_paths {
        match fs::symlink_metadata(retired_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect retired decision-evidence path `{}`",
                        retired_path.display()
                    )
                });
            }
            Ok(_) => bail!(
                "retired decision-evidence path still exists: `{}`",
                retired_path.display()
            ),
        }
    }
    Ok(())
}

fn scan_current_machine_stream(
    path: &Path,
    max_bytes: u64,
) -> Result<CurrentMachineStreamPreflight> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CurrentMachineStreamPreflight {
                records: 0,
                bytes: 0,
            });
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect current machine decision-evidence stream `{}`",
                    path.display()
                )
            });
        }
        Ok(_) => {}
    }
    let stream = read_registered_stream(path, max_bytes)
        .context("current machine decision-evidence stream is invalid")?;
    Ok(CurrentMachineStreamPreflight {
        records: stream.facts.len(),
        bytes: stream.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT_MACHINE_RECORD: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/submit_reservation_metadata_v1.jsonl"
    ));

    #[test]
    fn missing_machine_stream_is_a_valid_fresh_cutover_state() {
        let directory = tempfile::tempdir().unwrap();
        let result = scan_current_machine_stream(&directory.path().join("machine.jsonl"), 1)
            .expect("missing current stream should be accepted");
        assert_eq!(
            result,
            CurrentMachineStreamPreflight {
                records: 0,
                bytes: 0
            }
        );
    }

    #[test]
    fn any_configured_retired_path_refuses_activation() {
        let directory = tempfile::tempdir().unwrap();
        let retired_path = directory.path().join("order-intents.jsonl");
        fs::write(&retired_path, b"").unwrap();

        let error = ensure_retired_paths_absent(&[retired_path.clone()])
            .expect_err("retired stream presence must refuse activation");

        assert!(error.to_string().contains("retired decision-evidence path"));
        assert!(
            error
                .to_string()
                .contains(&retired_path.display().to_string())
        );
    }

    #[test]
    fn nonempty_foreign_stream_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.jsonl");
        fs::write(
            &path,
            b"{\"kind\":\"strategy_input_snapshot\",\"schema_version\":15}\n",
        )
        .unwrap();
        let error = scan_current_machine_stream(&path, 1024)
            .expect_err("pre-cutover identity must be rejected");
        assert!(error.to_string().contains("stream is invalid"));
    }

    #[test]
    fn registered_observation_identity_in_machine_stream_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.jsonl");
        let observation = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/entry_skip_observation_v1.jsonl"
        ));
        fs::write(&path, observation).unwrap();

        let error = scan_current_machine_stream(&path, observation.len() as u64)
            .expect_err("an observation identity in the machine stream must reject activation");

        assert!(format!("{error:#}").contains("contains observation identity"));
    }

    #[test]
    fn malformed_registered_current_record_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.jsonl");
        let mut record: serde_json::Value = serde_json::from_slice(CURRENT_MACHINE_RECORD).unwrap();
        record
            .as_object_mut()
            .expect("fixture must be an object")
            .insert("unregistered_field".to_string(), true.into());
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.push(b'\n');
        fs::write(&path, bytes).unwrap();

        let error = scan_current_machine_stream(&path, 4096)
            .expect_err("known identity with malformed current bytes must be rejected");

        assert!(format!("{error:#}").contains("unknown field"));
    }

    #[test]
    fn current_only_stream_is_accepted_at_the_exact_byte_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.jsonl");
        fs::write(&path, CURRENT_MACHINE_RECORD).unwrap();

        let result = scan_current_machine_stream(&path, CURRENT_MACHINE_RECORD.len() as u64)
            .expect("registered current record at the byte limit should be accepted");

        assert_eq!(result.records, 1);
        assert_eq!(result.bytes, CURRENT_MACHINE_RECORD.len() as u64);
    }

    #[test]
    fn current_only_stream_one_byte_over_the_limit_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.jsonl");
        fs::write(&path, CURRENT_MACHINE_RECORD).unwrap();

        let error = scan_current_machine_stream(
            &path,
            CURRENT_MACHINE_RECORD.len().saturating_sub(1) as u64,
        )
        .expect_err("stream above the byte limit must be rejected");

        assert!(error.to_string().contains("stream is invalid"));
        assert!(format!("{error:#}").contains("exceeds max_bytes"));
    }

    #[test]
    fn blank_record_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.jsonl");
        fs::write(&path, b"\n").unwrap();

        let error = scan_current_machine_stream(&path, 1)
            .expect_err("blank current record must be rejected");

        assert!(format!("{error:#}").contains("blank record"));
    }

    #[test]
    fn torn_stream_is_rejected_before_decoding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("machine.jsonl");
        fs::write(&path, b"{}").unwrap();
        let error = scan_current_machine_stream(&path, 1024)
            .expect_err("torn final record must be rejected");
        assert!(error.to_string().contains("torn final record"));
    }
}
