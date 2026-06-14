use std::{cell::RefCell, collections::BTreeMap, marker::PhantomData, rc::Rc};

use aws_config::BehaviorVersion;
use aws_sdk_ssm::{Client as SsmClient, config::Region};

#[derive(Debug)]
pub struct ArtifactStoreSecretError(String);

impl std::fmt::Display for ArtifactStoreSecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ArtifactStoreSecretError {}

pub trait ArtifactStoreSecretResolver {
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String>;
}

impl<F, E> ArtifactStoreSecretResolver for F
where
    F: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String> {
        self(region, ssm_path).map_err(|error| error.to_string())
    }
}

pub struct ArtifactStoreSsmResolver {
    runtime: tokio::runtime::Runtime,
    clients: RefCell<BTreeMap<String, SsmClient>>,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl ArtifactStoreSsmResolver {
    pub fn new() -> Result<Self, ArtifactStoreSecretError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                ArtifactStoreSecretError(format!(
                    "failed to build Tokio runtime for artifact-store SSM resolver: {error}"
                ))
            })?;
        Ok(Self {
            runtime,
            clients: RefCell::new(BTreeMap::new()),
            _not_send_sync: PhantomData,
        })
    }

    pub fn cached_region_count(&self) -> usize {
        self.clients.borrow().len()
    }

    pub fn resolve(
        &mut self,
        region: &str,
        ssm_path: &str,
    ) -> Result<String, ArtifactStoreSecretError> {
        Self::ensure_not_inside_active_tokio_runtime()?;
        let client = self.client_for(region)?;
        let ssm_path_owned = ssm_path.to_string();
        self.runtime.block_on(async move {
            let response = client
                .get_parameter()
                .name(&ssm_path_owned)
                .with_decryption(true)
                .send()
                .await
                .map_err(|error| {
                    let source = redact_configured_ssm_path(
                        &aws_sdk_ssm::error::DisplayErrorContext(&error).to_string(),
                        &ssm_path_owned,
                    );
                    ArtifactStoreSecretError(format!(
                        "AWS SSM GetParameter failed for configured artifact-store parameter: {source}"
                    ))
                })?;
            response
                .parameter()
                .and_then(|parameter| parameter.value())
                .map(str::to_string)
                .ok_or_else(|| {
                    ArtifactStoreSecretError(
                        "AWS SSM GetParameter returned no value for configured artifact-store parameter"
                            .to_string(),
                    )
                })
        })
    }

    fn ensure_not_inside_active_tokio_runtime() -> Result<(), ArtifactStoreSecretError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(ArtifactStoreSecretError(
                "ArtifactStoreSsmResolver invoked from inside an active Tokio runtime; \
                 artifact-store SSM resolution must run on the synchronous startup boundary"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn client_for(&self, region: &str) -> Result<SsmClient, ArtifactStoreSecretError> {
        Self::ensure_not_inside_active_tokio_runtime()?;
        if let Some(client) = self.clients.borrow().get(region) {
            return Ok(client.clone());
        }
        let region_owned = region.to_string();
        let aws_config = self.runtime.block_on(
            aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new(region_owned))
                .load(),
        );
        let client = SsmClient::new(&aws_config);
        self.clients
            .borrow_mut()
            .insert(region.to_string(), client.clone());
        Ok(client)
    }
}

impl ArtifactStoreSecretResolver for ArtifactStoreSsmResolver {
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String> {
        self.resolve(region, ssm_path)
            .map_err(|error| error.to_string())
    }
}

fn redact_configured_ssm_path(message: &str, configured_path: &str) -> String {
    if configured_path.is_empty() {
        return message.to_string();
    }
    message.replace(configured_path, "[configured-ssm-parameter]")
}
