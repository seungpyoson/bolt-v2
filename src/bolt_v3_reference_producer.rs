use nautilus_common::actor::DataActorConfig;
use nautilus_model::identifiers::{ClientId, InstrumentId};

use crate::{
    bolt_v3_config::{BoltV3RootConfig, ReferenceStreamBlock},
    bolt_v3_providers::{ProviderReferenceInputContext, binding_for_venue},
    config::ReferenceVenueEntry,
    platform::reference_actor::{ReferenceActor, ReferenceActorConfig, ReferenceSubscription},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3ReferenceProducerError {
    reason: String,
}

impl BoltV3ReferenceProducerError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for BoltV3ReferenceProducerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for BoltV3ReferenceProducerError {}

#[derive(Debug)]
pub struct BoltV3ReferenceActorPlan {
    pub config: ReferenceActorConfig,
    pub venue_cfgs: Vec<ReferenceVenueEntry>,
}

impl BoltV3ReferenceActorPlan {
    pub fn from_stream(
        root: &BoltV3RootConfig,
        stream_id: &str,
        stream: &ReferenceStreamBlock,
    ) -> Result<Self, BoltV3ReferenceProducerError> {
        let mut venue_subscriptions = Vec::with_capacity(stream.inputs.len());
        let mut venue_cfgs = Vec::with_capacity(stream.inputs.len());

        for (index, input) in stream.inputs.iter().enumerate() {
            let context = format!("reference_streams.{stream_id}.inputs[{index}]");
            let data_client_id = input.data_client_id.as_deref().ok_or_else(|| {
                BoltV3ReferenceProducerError::new(format!("{context}.data_client_id is required"))
            })?;
            let client = root.clients.get(data_client_id).ok_or_else(|| {
                BoltV3ReferenceProducerError::new(format!(
                    "{context}.data_client_id `{data_client_id}` does not match any [clients.<id>] block"
                ))
            })?;
            if client.data.is_none() {
                return Err(BoltV3ReferenceProducerError::new(format!(
                    "{context}.data_client_id `{data_client_id}` must reference a data-capable client"
                )));
            }

            let binding = binding_for_venue(client.venue.as_str()).ok_or_else(|| {
                BoltV3ReferenceProducerError::new(format!(
                    "{context}.data_client_id `{data_client_id}` uses unsupported client venue `{}`",
                    client.venue.as_str()
                ))
            })?;
            let build_reference_venue_entry =
                binding.build_reference_venue_entry.ok_or_else(|| {
                    BoltV3ReferenceProducerError::new(format!(
                        "{context}.data_client_id `{data_client_id}` uses client venue `{}` which does not support reference stream inputs",
                        client.venue.as_str()
                    ))
                })?;
            venue_subscriptions.push(ReferenceSubscription {
                venue_name: input.source_id.clone(),
                instrument_id: InstrumentId::from(input.instrument_id.as_str()),
                client_id: ClientId::from(data_client_id),
            });
            venue_cfgs.push(
                build_reference_venue_entry(ProviderReferenceInputContext {
                    stream_id,
                    input_index: index,
                    input,
                })
                .map_err(BoltV3ReferenceProducerError::new)?,
            );
        }

        Ok(Self {
            config: ReferenceActorConfig {
                base: DataActorConfig::default(),
                publish_topic: stream.publish_topic.clone(),
                min_publish_interval_ms: stream.min_publish_interval_milliseconds,
                venue_subscriptions,
            },
            venue_cfgs,
        })
    }

    pub fn into_actor(self) -> ReferenceActor {
        ReferenceActor::new(self.config, self.venue_cfgs)
    }
}
