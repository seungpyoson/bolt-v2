use std::collections::BTreeMap;

use crate::{
    bolt_v3_config::{ReferenceSourceType, ReferenceStreamBlock},
    platform::reference::{
        ReferenceFusionInput, ReferenceObservation, ReferenceSnapshot, VenueKind,
        derive_reference_disabled_sources_from_inputs, fuse_reference_snapshot_from_inputs,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3ReferenceStreamPolicy {
    stream_id: String,
    publish_topic: String,
    inputs: Vec<ReferenceFusionInput>,
}

impl BoltV3ReferenceStreamPolicy {
    pub fn from_stream(stream_id: &str, stream: &ReferenceStreamBlock) -> Result<Self, String> {
        if stream_id.trim().is_empty() {
            return Err("reference stream id must be non-empty".to_string());
        }
        if stream.publish_topic.trim().is_empty() {
            return Err(format!(
                "reference_streams.{stream_id}.publish_topic must be non-empty"
            ));
        }
        if stream.inputs.is_empty() {
            return Err(format!(
                "reference_streams.{stream_id}.inputs must list at least one input"
            ));
        }

        let inputs = stream
            .inputs
            .iter()
            .map(|input| ReferenceFusionInput {
                source_id: input.source_id.clone(),
                source_type: match input.source_type {
                    ReferenceSourceType::Oracle => VenueKind::Oracle,
                    ReferenceSourceType::Orderbook => VenueKind::Orderbook,
                },
                instrument_id: input.instrument_id.clone(),
                base_weight: input.base_weight,
                stale_after_ms: input.stale_after_milliseconds,
                disable_after_ms: input.disable_after_milliseconds,
            })
            .collect();

        Ok(Self {
            stream_id: stream_id.to_string(),
            publish_topic: stream.publish_topic.clone(),
            inputs,
        })
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn publish_topic(&self) -> &str {
        &self.publish_topic
    }

    pub fn fuse_snapshot(
        &self,
        now_ms: u64,
        latest: &BTreeMap<String, ReferenceObservation>,
        disabled: &BTreeMap<String, String>,
    ) -> ReferenceSnapshot {
        fuse_reference_snapshot_from_inputs(
            &self.publish_topic,
            now_ms,
            &self.inputs,
            latest,
            disabled,
        )
    }

    pub fn disabled_sources(
        &self,
        now_ms: u64,
        latest: &BTreeMap<String, ReferenceObservation>,
    ) -> BTreeMap<String, String> {
        derive_reference_disabled_sources_from_inputs(now_ms, &self.inputs, latest)
    }

    pub fn fuse_snapshot_with_source_health(
        &self,
        now_ms: u64,
        latest: &BTreeMap<String, ReferenceObservation>,
    ) -> ReferenceSnapshot {
        let disabled = self.disabled_sources(now_ms, latest);

        self.fuse_snapshot(now_ms, latest, &disabled)
    }
}
