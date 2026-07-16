//! Source-object transports selected by the batch launch configuration.
//!
//! A transport is an explicit policy choice. The staged-S3 implementation
//! never falls back to a provider URL: it derives the one configured store
//! from the verified RunSpec, resolves its credentials only from the RunSpec's
//! SSM parameter references, discovers the current non-null S3 version, and
//! reads that exact version. The transport returns only an opaque object whose
//! execution-pack byte-length and SHA-256 identity it has already verified.

use std::time::Duration;

use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use object_store::{GetOptions, ObjectStore, ObjectStoreExt, path::Path as ObjectPath};

use crate::{
    artifact_store::{ResolvedArtifactRoot, ensure_immutable_s3_version_id},
    nt_catalog_capability::{NtCatalogSsmCredentialResolver, NtCatalogSsmParameterRefs},
    operator::RunSpec,
    operator_work_budget::{
        ExactSizedObjectBuffer, OperatorWorkBudgetGuard, OperatorWorkBudgetStage,
        guarded_async_operation_outcome,
    },
    source_universe_batch_execution::{SourceUniverseObjectFetcher, VerifiedSourceObject},
    source_universe_execution_pack::SourceUniverseExecutionPackRecord,
};

#[derive(Debug)]
struct StagedS3ReadPlan {
    artifact_root: ResolvedArtifactRoot,
    object_path: ObjectPath,
    credential_region: String,
    credential_parameters: NtCatalogSsmParameterRefs,
}

/// Exact-current-version reader for execution-pack objects already staged in
/// the configured canonical S3 artifact store.
pub struct StagedS3SourceUniverseObjectFetcher {
    runtime: tokio::runtime::Runtime,
    fetch_timeout: Option<Duration>,
}

impl StagedS3SourceUniverseObjectFetcher {
    /// Construct one staged-S3 reader. A configured timeout must be positive
    /// and bounds SSM preparation plus the complete S3 read.
    pub fn new(fetch_timeout_seconds: Option<u64>) -> Result<Self> {
        if let Some(seconds) = fetch_timeout_seconds {
            ensure!(seconds > 0, "fetch_timeout_seconds must be positive");
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("create staged-S3 fetch runtime")?;
        Ok(Self {
            runtime,
            fetch_timeout: fetch_timeout_seconds.map(Duration::from_secs),
        })
    }
}

impl SourceUniverseObjectFetcher for StagedS3SourceUniverseObjectFetcher {
    fn fetch(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        run_spec: &RunSpec,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<VerifiedSourceObject> {
        let plan = staged_s3_read_plan(record, run_spec)?;
        let operation = fetch_staged_s3_exact_current_version(record, plan, work_budget);
        let result = match self.fetch_timeout {
            Some(timeout) => self.runtime.block_on(async {
                tokio::time::timeout(timeout, operation)
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "fetch_timeout_seconds exhausted after {timeout:?} while reading staged S3 source"
                        )
                    })?
            }),
            None => self.runtime.block_on(operation),
        };
        work_budget.check_deadline(OperatorWorkBudgetStage::Fetch)?;
        let bytes = result?;
        VerifiedSourceObject::verify(record, bytes, work_budget)
    }
}

fn staged_s3_read_plan(
    record: &SourceUniverseExecutionPackRecord,
    run_spec: &RunSpec,
) -> Result<StagedS3ReadPlan> {
    ensure!(
        record.source_uri == run_spec.accepted_object.s3_uri
            && record.source_uri == run_spec.source_proof.raw_sample_uri,
        "staged-S3 transport source_uri does not match the verified RunSpec"
    );
    ensure!(
        record.source_url == run_spec.accepted_object.source_url,
        "staged-S3 transport provider source_url does not match the verified RunSpec"
    );
    ensure!(
        record.selected_object_bytes == run_spec.accepted_object.bytes
            && record.selected_object_sha256 == run_spec.accepted_object.sha256
            && record.selected_object_sha256 == run_spec.source_proof.raw_sample_hash,
        "staged-S3 transport object identity does not match the verified RunSpec"
    );
    let artifact_store = run_spec.required_artifact_store()?;
    run_spec.validate_artifact_store_publish_config(artifact_store)?;
    let artifact_root = artifact_store.resolve()?;
    let object_path = artifact_root
        .object_path_for_same_bucket_uri(&record.source_uri)
        .context("resolve staged source in configured artifact bucket")?;
    let parameters = run_spec
        .manifest
        .artifact_store
        .ssm_parameters
        .as_ref()
        .context("staged-S3 transport requires manifest SSM credential parameters")?;
    ensure!(
        parameters.region == artifact_root.s3_region(),
        "staged-S3 transport SSM region must match artifact-store S3 region"
    );
    Ok(StagedS3ReadPlan {
        artifact_root,
        object_path,
        credential_region: parameters.region.clone(),
        credential_parameters: NtCatalogSsmParameterRefs {
            access_key_id: parameters.access_key_id.clone(),
            secret_access_key: parameters.secret_access_key.clone(),
            session_token: parameters.session_token.clone(),
        },
    })
}

async fn fetch_staged_s3_exact_current_version(
    record: &SourceUniverseExecutionPackRecord,
    plan: StagedS3ReadPlan,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<u8>> {
    let resolver = guarded_async_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Fetch,
        NtCatalogSsmCredentialResolver::from_region(&plan.credential_region),
    )
    .await??;
    let credentials = guarded_async_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Fetch,
        resolver.resolve(&plan.credential_parameters),
    )
    .await??;
    let store = plan
        .artifact_root
        .build_s3_object_store_with_credentials(&credentials)?;
    read_staged_s3_exact_current_version(
        &store,
        &plan.object_path,
        record.selected_object_bytes,
        &record.source_uri,
        work_budget,
    )
    .await
}

async fn read_staged_s3_exact_current_version(
    store: &dyn ObjectStore,
    object_path: &ObjectPath,
    expected_bytes: u64,
    source_uri: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<u8>> {
    let current = guarded_async_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Fetch,
        store.head(object_path),
    )
    .await?
    .with_context(|| format!("discover current staged S3 source at {source_uri}"))?;
    ensure!(
        current.location == *object_path,
        "current staged S3 source returned a different object path"
    );
    ensure!(
        current.size == expected_bytes,
        "current staged S3 source size {} does not match pinned {}",
        current.size,
        expected_bytes
    );
    let version_id = current
        .version
        .clone()
        .context("current staged S3 source has no version ID")?;
    ensure_immutable_s3_version_id("current staged S3 source version ID", &version_id)?;
    let exact = guarded_async_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Fetch,
        store.get_opts(
            object_path,
            GetOptions {
                version: Some(version_id.clone()),
                if_match: current.e_tag.clone(),
                ..GetOptions::default()
            },
        ),
    )
    .await?
    .with_context(|| format!("read exact staged S3 source version at {source_uri}"))?;
    ensure!(
        exact.meta.location == *object_path,
        "exact staged S3 source returned a different object path"
    );
    ensure!(
        exact.meta.size == expected_bytes,
        "exact staged S3 source size {} does not match pinned {expected_bytes}",
        exact.meta.size
    );
    ensure!(
        exact.range.start == 0 && exact.range.end == expected_bytes,
        "exact staged S3 source range {:?} is not 0..{expected_bytes}",
        exact.range
    );
    ensure!(
        exact.meta.version.as_deref() == Some(version_id.as_str()),
        "exact staged S3 source version metadata mismatch"
    );
    if let Some(expected_e_tag) = &current.e_tag {
        ensure!(
            exact.meta.e_tag.as_ref() == Some(expected_e_tag),
            "exact staged S3 source ETag mismatch"
        );
    }

    let mut output = ExactSizedObjectBuffer::new(expected_bytes)?;
    let mut stream = exact.into_stream();
    loop {
        let chunk =
            guarded_async_operation_outcome(work_budget, OperatorWorkBudgetStage::Fetch, async {
                stream.next().await.transpose()
            })
            .await?
            .context("stream exact staged S3 source")?;
        let Some(chunk) = chunk else { break };
        output.push(&chunk, work_budget, OperatorWorkBudgetStage::Fetch)?;
    }
    output.finish(work_budget, OperatorWorkBudgetStage::Fetch)
}

#[cfg(test)]
mod tests {
    use std::{
        fmt, fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use bytes::Bytes;
    use futures_util::{StreamExt, stream::BoxStream};
    use object_store::{
        Attributes, CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult,
        MultipartUpload, ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload,
        PutResult, Result as ObjectStoreResult, memory::InMemory, path::Path as ObjectPath,
    };

    use super::{read_staged_s3_exact_current_version, staged_s3_read_plan};
    use crate::{
        operator::RunSpec, operator_work_budget::OperatorWorkBudgetGuard,
        source_universe_execution_pack::SourceUniverseExecutionPack,
    };
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    use crate::source_universe_batch_launch::discover_committed_source_universe_execution_packs;

    #[derive(Debug)]
    struct ExactVersionReadStore {
        inner: InMemory,
        location: ObjectPath,
        payload: Bytes,
        head_version: Option<String>,
        exact_response_version: Option<String>,
        e_tag: String,
        exact_response_e_tag: String,
        reported_size: u64,
        exact_gets: AtomicUsize,
    }

    impl ExactVersionReadStore {
        fn new(payload: &[u8]) -> Self {
            Self {
                inner: InMemory::new(),
                location: ObjectPath::from("backfill-staging/object.bin"),
                payload: Bytes::copy_from_slice(payload),
                head_version: Some("source-version-1".to_string()),
                exact_response_version: Some("source-version-1".to_string()),
                e_tag: "source-etag-1".to_string(),
                exact_response_e_tag: "source-etag-1".to_string(),
                reported_size: u64::try_from(payload.len()).expect("test payload size fits u64"),
                exact_gets: AtomicUsize::new(0),
            }
        }

        fn result(
            &self,
            version: Option<String>,
            e_tag: String,
            include_payload: bool,
        ) -> GetResult {
            let payload = include_payload
                .then(|| self.payload.clone())
                .unwrap_or_default();
            let size = self.reported_size;
            GetResult {
                payload: GetResultPayload::Stream(
                    futures_util::stream::once(
                        async move { Ok::<_, object_store::Error>(payload) },
                    )
                    .boxed(),
                ),
                meta: ObjectMeta {
                    location: self.location.clone(),
                    last_modified: chrono::Utc::now(),
                    size,
                    e_tag: Some(e_tag),
                    version,
                },
                range: 0..size,
                attributes: Attributes::default(),
            }
        }
    }

    impl fmt::Display for ExactVersionReadStore {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("ExactVersionReadStore")
        }
    }

    #[async_trait::async_trait]
    impl ObjectStore for ExactVersionReadStore {
        async fn put_opts(
            &self,
            location: &ObjectPath,
            payload: PutPayload,
            options: PutOptions,
        ) -> ObjectStoreResult<PutResult> {
            self.inner.put_opts(location, payload, options).await
        }

        async fn put_multipart_opts(
            &self,
            location: &ObjectPath,
            options: PutMultipartOptions,
        ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
            self.inner.put_multipart_opts(location, options).await
        }

        async fn get_opts(
            &self,
            location: &ObjectPath,
            options: GetOptions,
        ) -> ObjectStoreResult<GetResult> {
            if location != &self.location {
                return Err(object_store::Error::NotFound {
                    path: location.to_string(),
                    source: Box::new(std::io::Error::from(std::io::ErrorKind::NotFound)),
                });
            }
            if options.head {
                return Ok(self.result(self.head_version.clone(), self.e_tag.clone(), false));
            }
            self.exact_gets.fetch_add(1, Ordering::SeqCst);
            if options.version.as_deref() != self.head_version.as_deref()
                || options.if_match.as_deref() != Some(self.e_tag.as_str())
            {
                return Err(object_store::Error::Precondition {
                    path: location.to_string(),
                    source: Box::new(std::io::Error::other(
                        "exact version or ETag was not requested",
                    )),
                });
            }
            Ok(self.result(
                self.exact_response_version.clone(),
                self.exact_response_e_tag.clone(),
                true,
            ))
        }

        fn delete_stream(
            &self,
            locations: BoxStream<'static, ObjectStoreResult<ObjectPath>>,
        ) -> BoxStream<'static, ObjectStoreResult<ObjectPath>> {
            self.inner.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&ObjectPath>,
        ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
            self.inner.list(prefix)
        }

        fn list_with_offset(
            &self,
            prefix: Option<&ObjectPath>,
            offset: &ObjectPath,
        ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
            self.inner.list_with_offset(prefix, offset)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&ObjectPath>,
        ) -> ObjectStoreResult<ListResult> {
            self.inner.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &ObjectPath,
            to: &ObjectPath,
            options: CopyOptions,
        ) -> ObjectStoreResult<()> {
            self.inner.copy_opts(from, to, options).await
        }
    }

    #[test]
    fn staged_s3_read_uses_current_identity_for_one_exact_version_get() {
        let store = ExactVersionReadStore::new(b"pinned-source");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let bytes = runtime
            .block_on(read_staged_s3_exact_current_version(
                &store,
                &store.location,
                13,
                "s3://bucket/backfill-staging/object.bin",
                &OperatorWorkBudgetGuard::unbounded(),
            ))
            .expect("exact current-version read");
        assert_eq!(bytes, b"pinned-source");
        assert_eq!(store.exact_gets.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn staged_s3_read_rejects_null_or_mismatched_version_metadata() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        for store in [
            ExactVersionReadStore {
                head_version: Some("null".to_string()),
                ..ExactVersionReadStore::new(b"pinned-source")
            },
            ExactVersionReadStore {
                exact_response_version: Some("different-version".to_string()),
                ..ExactVersionReadStore::new(b"pinned-source")
            },
        ] {
            let error = runtime
                .block_on(read_staged_s3_exact_current_version(
                    &store,
                    &store.location,
                    13,
                    "s3://bucket/backfill-staging/object.bin",
                    &OperatorWorkBudgetGuard::unbounded(),
                ))
                .expect_err("invalid version metadata must fail closed");
            assert!(error.to_string().contains("version"), "{error:#}");
        }
    }

    #[test]
    fn staged_s3_read_rejects_etag_and_body_length_drift() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        for store in [
            ExactVersionReadStore {
                exact_response_e_tag: "different-etag".to_string(),
                ..ExactVersionReadStore::new(b"pinned-source")
            },
            ExactVersionReadStore {
                reported_size: 13,
                ..ExactVersionReadStore::new(b"short")
            },
            ExactVersionReadStore {
                reported_size: 13,
                ..ExactVersionReadStore::new(b"pinned-source!")
            },
        ] {
            runtime
                .block_on(read_staged_s3_exact_current_version(
                    &store,
                    &store.location,
                    13,
                    "s3://bucket/backfill-staging/object.bin",
                    &OperatorWorkBudgetGuard::unbounded(),
                ))
                .expect_err("ETag, short body, and long body drift must fail closed");
        }
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn committed_tracers_plan_only_their_staged_s3_object() {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let committed_packs =
            discover_committed_source_universe_execution_packs(&repository_root)
                .expect("discover committed execution packs");
        for committed_pack in committed_packs {
            let pack: SourceUniverseExecutionPack = serde_json::from_slice(
                &fs::read(&committed_pack.summary_path).unwrap_or_else(|error| {
                    panic!(
                        "read committed execution pack {}: {error}",
                        committed_pack.summary_path.display()
                    )
                }),
            )
            .expect("parse committed execution pack");
            let mut record = pack.records.first().expect("one committed tracer").clone();
            let run_spec: RunSpec = toml::from_slice(
                &fs::read(repository_root.join(&record.run_spec_path))
                    .expect("read committed tracer RunSpec"),
            )
            .expect("parse committed tracer RunSpec");

            let plan = staged_s3_read_plan(&record, &run_spec)
                .expect("committed staged-S3 transport plan");
            assert!(plan.object_path.as_ref().contains("backfill-staging"));

            record.source_uri = record.source_url.clone();
            let error = staged_s3_read_plan(&record, &run_spec)
                .expect_err("provider URL cannot substitute for the staged S3 URI");
            assert!(error.to_string().contains("source_uri"), "{error:#}");

            let mut cross_bucket_record = pack.records[0].clone();
            let mut cross_bucket_spec = run_spec.clone();
            let cross_bucket_uri = cross_bucket_record.source_uri.replacen(
                "s3://bolt-parquet/",
                "s3://different-bucket/",
                1,
            );
            cross_bucket_record.source_uri.clone_from(&cross_bucket_uri);
            cross_bucket_spec
                .accepted_object
                .s3_uri
                .clone_from(&cross_bucket_uri);
            cross_bucket_spec
                .source_proof
                .raw_sample_uri
                .clone_from(&cross_bucket_uri);
            let error = staged_s3_read_plan(&cross_bucket_record, &cross_bucket_spec)
                .expect_err("a staged source in a different bucket must fail closed");
            assert!(error.to_string().contains("bucket"), "{error:#}");

            let mut missing_ssm_spec = run_spec.clone();
            missing_ssm_spec.manifest.artifact_store.ssm_parameters = None;
            let error = staged_s3_read_plan(&pack.records[0], &missing_ssm_spec)
                .expect_err("missing SSM refs must fail during the network-free plan");
            assert!(error.to_string().contains("SSM"), "{error:#}");
        }
    }
}
