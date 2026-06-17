//! Fail-closed go-live gate for the built maker backtest evidence.
//!
//! This module is intentionally venue-agnostic: it evaluates whether the maker
//! was backtested as built, against pre-registered thresholds, on queue-aware
//! historical market data, using the shared pricing and settlement primitives.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerBacktestVerdict {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerBacktestEvidence {
    pub verdict: MakerBacktestVerdict,
    pub build_head_sha_valid: bool,
    pub strategy_config_hash_valid: bool,
    pub result_contract_strategy_config_hash_valid: bool,
    pub run_artifact_present: bool,
    pub run_artifact_sha256_valid: bool,
    pub threshold_artifact_present: bool,
    pub threshold_artifact_sha256_valid: bool,
    pub execution_model_artifact_present: bool,
    pub execution_model_artifact_sha256_valid: bool,
    pub maker_orders_observed: bool,
    pub passive_fills_observed: bool,
    pub built_maker_replayed: bool,
    pub full_net_scoring: bool,
    pub thresholds_registered_before_run: bool,
    pub balanced_gate_evaluated: bool,
    pub strict_gate_evaluated: bool,
    pub balanced_gate_passed: bool,
    pub strict_gate_passed: bool,
    pub historical_full_depth_l2: bool,
    pub full_population_corpus: bool,
    pub entry_gated_corpus_used: bool,
    pub trade_ticks_present: bool,
    pub order_book_deltas_present: bool,
    pub queue_position_enabled: bool,
    pub nt_execution_model_used: bool,
    pub custom_fill_model_used: bool,
    pub custom_fill_model_source_proven: bool,
    pub underlying_spot_causal_join: bool,
    pub net_edge_positive: bool,
    pub statistical_significance: bool,
    pub passive_fill_power_floor: bool,
    pub resolved_market_corpus_floor: bool,
    pub shared_fair_value_pricing: bool,
    pub shared_settlement_primitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerBacktestGateBlocker {
    VerdictNotPass,
    MissingBuildHeadSha,
    MissingStrategyConfigHash,
    ResultContractStrategyConfigHashMismatch,
    MissingRunArtifact,
    MissingRunArtifactDigest,
    MissingThresholdArtifact,
    MissingThresholdArtifactDigest,
    MissingExecutionModelArtifact,
    MissingExecutionModelArtifactDigest,
    MissingMakerOrders,
    MissingPassiveFills,
    BuiltMakerNotReplayed,
    MissingFullNetScoring,
    ThresholdsNotPreRegistered,
    BalancedGateNotEvaluated,
    StrictGateNotEvaluated,
    BalancedGateNotPassed,
    MissingHistoricalFullDepthL2,
    MissingFullPopulationCorpus,
    EntryGatedCorpusUsed,
    MissingTradeTicks,
    MissingOrderBookDeltas,
    QueuePositionDisabled,
    MissingNtExecutionModel,
    CustomFillModelWithoutSourceProof,
    MissingUnderlyingSpotCausalJoin,
    NetEdgeNotPositive,
    MissingStatisticalSignificance,
    MissingPassiveFillPowerFloor,
    MissingResolvedMarketCorpusFloor,
    MissingSharedFairValuePricing,
    MissingSharedSettlementPrimitive,
}

impl MakerBacktestGateBlocker {
    pub fn parameter_path(self) -> &'static str {
        match self {
            Self::VerdictNotPass => "verdict",
            Self::MissingBuildHeadSha => "build_head_sha",
            Self::MissingStrategyConfigHash => "strategy_config_hash",
            Self::ResultContractStrategyConfigHashMismatch => {
                "result_contract_replay.strategy_config_hash"
            }
            Self::MissingRunArtifact => "run_artifact",
            Self::MissingRunArtifactDigest => "run_artifact_sha256",
            Self::MissingThresholdArtifact => "threshold_artifact",
            Self::MissingThresholdArtifactDigest => "threshold_artifact_sha256",
            Self::MissingExecutionModelArtifact => "execution_model_artifact",
            Self::MissingExecutionModelArtifactDigest => "execution_model_artifact_sha256",
            Self::MissingMakerOrders => "maker_order_count",
            Self::MissingPassiveFills => "passive_fill_count",
            Self::BuiltMakerNotReplayed => "result_contract_replay.strategy_registry_key",
            Self::MissingFullNetScoring => "net_score_micros",
            Self::ThresholdsNotPreRegistered => "thresholds_registered_before_run",
            Self::BalancedGateNotEvaluated => "balanced_gate_evaluated",
            Self::StrictGateNotEvaluated => "strict_gate_evaluated",
            Self::BalancedGateNotPassed => "balanced_gate_passed",
            Self::MissingHistoricalFullDepthL2 => "historical_full_depth_l2",
            Self::MissingFullPopulationCorpus => "full_population_corpus",
            Self::EntryGatedCorpusUsed => "entry_gated_corpus_used",
            Self::MissingTradeTicks => "result_contract_replay.catalog_data_types",
            Self::MissingOrderBookDeltas => "result_contract_replay.catalog_data_types",
            Self::QueuePositionDisabled => "result_contract_replay.venue_queue_position",
            Self::MissingNtExecutionModel => "result_contract_replay.execution_model",
            Self::CustomFillModelWithoutSourceProof => "custom_fill_model_source_proven",
            Self::MissingUnderlyingSpotCausalJoin => "underlying_spot_causal_join",
            Self::NetEdgeNotPositive => "net_score_micros",
            Self::MissingStatisticalSignificance => "statistical_significance",
            Self::MissingPassiveFillPowerFloor => "passive_fill_count",
            Self::MissingResolvedMarketCorpusFloor => "resolved_market_count",
            Self::MissingSharedFairValuePricing => "shared_fair_value_pricing",
            Self::MissingSharedSettlementPrimitive => "shared_settlement_primitive",
        }
    }

    pub fn required_state(self) -> &'static str {
        match self {
            Self::VerdictNotPass => "must be `pass` before maker go-live",
            Self::MissingBuildHeadSha => {
                "must match this binary's lowercase Git head SHA for the maker build replayed by the backtest"
            }
            Self::MissingStrategyConfigHash => {
                "must supply the lowercase SHA-256 strategy config hash replayed by the backtest"
            }
            Self::ResultContractStrategyConfigHashMismatch => {
                "must match the BTE result-contract strategy_config_hash for the replayed run"
            }
            Self::MissingRunArtifact => "must name the immutable backtest run evidence artifact",
            Self::MissingRunArtifactDigest => {
                "must supply the lowercase SHA-256 digest for the backtest run artifact"
            }
            Self::MissingThresholdArtifact => {
                "must name the immutable pre-registered threshold artifact"
            }
            Self::MissingThresholdArtifactDigest => {
                "must supply the lowercase SHA-256 digest for the pre-registered threshold artifact"
            }
            Self::MissingExecutionModelArtifact => {
                "must name the immutable NT execution-model source evidence artifact"
            }
            Self::MissingExecutionModelArtifactDigest => {
                "must supply the lowercase SHA-256 digest for the NT execution-model source evidence artifact"
            }
            Self::MissingMakerOrders => {
                "must prove the built maker placed at least one order in the backtest"
            }
            Self::MissingPassiveFills => {
                "must prove the built maker observed at least one passive fill in the backtest"
            }
            Self::BuiltMakerNotReplayed => {
                "must match the built maker strategy registry key replayed by BTE"
            }
            Self::MissingFullNetScoring => {
                "must reconcile net_score_micros = captured_spread_score_micros - fees_score_micros - adverse_selection_score_micros - settlement_loss_score_micros"
            }
            Self::ThresholdsNotPreRegistered => {
                "must confirm thresholds were registered before scoring"
            }
            Self::BalancedGateNotEvaluated => "must confirm the balanced gate was evaluated",
            Self::StrictGateNotEvaluated => "must confirm the strict gate was evaluated",
            Self::BalancedGateNotPassed => "must confirm at least the balanced gate passed",
            Self::MissingHistoricalFullDepthL2 => {
                "must confirm the corpus is historical full-depth L2"
            }
            Self::MissingFullPopulationCorpus => "must confirm the full population corpus was used",
            Self::EntryGatedCorpusUsed => "must be false; entry-gated evidence is selection-biased",
            Self::MissingTradeTicks => {
                "must include TradeTick in the BTE result-contract catalog_data_types"
            }
            Self::MissingOrderBookDeltas => {
                "must include OrderBookDelta in the BTE result-contract catalog_data_types"
            }
            Self::QueuePositionDisabled => {
                "must confirm BTE result-contract venue_queue_position is enabled"
            }
            Self::MissingNtExecutionModel => {
                "must confirm the BTE result-contract execution_model is NT BacktestNode"
            }
            Self::CustomFillModelWithoutSourceProof => {
                "must be true when a custom fill model is used"
            }
            Self::MissingUnderlyingSpotCausalJoin => {
                "must confirm the underlying spot join is point-in-time causal"
            }
            Self::NetEdgeNotPositive => "must be positive after full net scoring reconciliation",
            Self::MissingStatisticalSignificance => {
                "must confirm statistical significance for net edge > 0"
            }
            Self::MissingPassiveFillPowerFloor => {
                "must meet a non-zero pre-registered passive-fill count floor"
            }
            Self::MissingResolvedMarketCorpusFloor => {
                "must meet a non-zero pre-registered resolved-market count floor"
            }
            Self::MissingSharedFairValuePricing => {
                "must confirm shared fair-value pricing was used"
            }
            Self::MissingSharedSettlementPrimitive => {
                "must confirm shared settlement accounting was used"
            }
        }
    }
}

pub fn maker_backtest_gate_blockers(
    evidence: &MakerBacktestEvidence,
) -> Vec<MakerBacktestGateBlocker> {
    let mut blockers = Vec::new();
    if evidence.verdict != MakerBacktestVerdict::Pass {
        blockers.push(MakerBacktestGateBlocker::VerdictNotPass);
    }
    if !evidence.build_head_sha_valid {
        blockers.push(MakerBacktestGateBlocker::MissingBuildHeadSha);
    }
    if !evidence.strategy_config_hash_valid {
        blockers.push(MakerBacktestGateBlocker::MissingStrategyConfigHash);
    }
    if !evidence.result_contract_strategy_config_hash_valid {
        blockers.push(MakerBacktestGateBlocker::ResultContractStrategyConfigHashMismatch);
    }
    if !evidence.run_artifact_present {
        blockers.push(MakerBacktestGateBlocker::MissingRunArtifact);
    }
    if !evidence.run_artifact_sha256_valid {
        blockers.push(MakerBacktestGateBlocker::MissingRunArtifactDigest);
    }
    if !evidence.threshold_artifact_present {
        blockers.push(MakerBacktestGateBlocker::MissingThresholdArtifact);
    }
    if !evidence.threshold_artifact_sha256_valid {
        blockers.push(MakerBacktestGateBlocker::MissingThresholdArtifactDigest);
    }
    if !evidence.execution_model_artifact_present {
        blockers.push(MakerBacktestGateBlocker::MissingExecutionModelArtifact);
    }
    if !evidence.execution_model_artifact_sha256_valid {
        blockers.push(MakerBacktestGateBlocker::MissingExecutionModelArtifactDigest);
    }
    if !evidence.maker_orders_observed {
        blockers.push(MakerBacktestGateBlocker::MissingMakerOrders);
    }
    if !evidence.passive_fills_observed {
        blockers.push(MakerBacktestGateBlocker::MissingPassiveFills);
    }
    if !evidence.built_maker_replayed {
        blockers.push(MakerBacktestGateBlocker::BuiltMakerNotReplayed);
    }
    if !evidence.full_net_scoring {
        blockers.push(MakerBacktestGateBlocker::MissingFullNetScoring);
    }
    if !evidence.thresholds_registered_before_run {
        blockers.push(MakerBacktestGateBlocker::ThresholdsNotPreRegistered);
    }
    if !evidence.balanced_gate_evaluated {
        blockers.push(MakerBacktestGateBlocker::BalancedGateNotEvaluated);
    }
    if !evidence.strict_gate_evaluated {
        blockers.push(MakerBacktestGateBlocker::StrictGateNotEvaluated);
    }
    if !evidence.balanced_gate_passed {
        blockers.push(MakerBacktestGateBlocker::BalancedGateNotPassed);
    }
    if !evidence.historical_full_depth_l2 {
        blockers.push(MakerBacktestGateBlocker::MissingHistoricalFullDepthL2);
    }
    if !evidence.full_population_corpus {
        blockers.push(MakerBacktestGateBlocker::MissingFullPopulationCorpus);
    }
    if evidence.entry_gated_corpus_used {
        blockers.push(MakerBacktestGateBlocker::EntryGatedCorpusUsed);
    }
    if !evidence.trade_ticks_present {
        blockers.push(MakerBacktestGateBlocker::MissingTradeTicks);
    }
    if !evidence.order_book_deltas_present {
        blockers.push(MakerBacktestGateBlocker::MissingOrderBookDeltas);
    }
    if !evidence.queue_position_enabled {
        blockers.push(MakerBacktestGateBlocker::QueuePositionDisabled);
    }
    if !evidence.nt_execution_model_used {
        blockers.push(MakerBacktestGateBlocker::MissingNtExecutionModel);
    }
    if evidence.custom_fill_model_used && !evidence.custom_fill_model_source_proven {
        blockers.push(MakerBacktestGateBlocker::CustomFillModelWithoutSourceProof);
    }
    if !evidence.underlying_spot_causal_join {
        blockers.push(MakerBacktestGateBlocker::MissingUnderlyingSpotCausalJoin);
    }
    if !evidence.net_edge_positive {
        blockers.push(MakerBacktestGateBlocker::NetEdgeNotPositive);
    }
    if !evidence.statistical_significance {
        blockers.push(MakerBacktestGateBlocker::MissingStatisticalSignificance);
    }
    if !evidence.passive_fill_power_floor {
        blockers.push(MakerBacktestGateBlocker::MissingPassiveFillPowerFloor);
    }
    if !evidence.resolved_market_corpus_floor {
        blockers.push(MakerBacktestGateBlocker::MissingResolvedMarketCorpusFloor);
    }
    if !evidence.shared_fair_value_pricing {
        blockers.push(MakerBacktestGateBlocker::MissingSharedFairValuePricing);
    }
    if !evidence.shared_settlement_primitive {
        blockers.push(MakerBacktestGateBlocker::MissingSharedSettlementPrimitive);
    }
    blockers
}

pub fn maker_backtest_gate_passes(evidence: &MakerBacktestEvidence) -> bool {
    maker_backtest_gate_blockers(evidence).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_evidence() -> MakerBacktestEvidence {
        MakerBacktestEvidence {
            verdict: MakerBacktestVerdict::Pass,
            build_head_sha_valid: true,
            strategy_config_hash_valid: true,
            result_contract_strategy_config_hash_valid: true,
            run_artifact_present: true,
            run_artifact_sha256_valid: true,
            threshold_artifact_present: true,
            threshold_artifact_sha256_valid: true,
            execution_model_artifact_present: true,
            execution_model_artifact_sha256_valid: true,
            maker_orders_observed: true,
            passive_fills_observed: true,
            built_maker_replayed: true,
            full_net_scoring: true,
            thresholds_registered_before_run: true,
            balanced_gate_evaluated: true,
            strict_gate_evaluated: true,
            balanced_gate_passed: true,
            strict_gate_passed: false,
            historical_full_depth_l2: true,
            full_population_corpus: true,
            entry_gated_corpus_used: false,
            trade_ticks_present: true,
            order_book_deltas_present: true,
            queue_position_enabled: true,
            nt_execution_model_used: true,
            custom_fill_model_used: false,
            custom_fill_model_source_proven: false,
            underlying_spot_causal_join: true,
            net_edge_positive: true,
            statistical_significance: true,
            passive_fill_power_floor: true,
            resolved_market_corpus_floor: true,
            shared_fair_value_pricing: true,
            shared_settlement_primitive: true,
        }
    }

    #[test]
    fn passing_evidence_has_no_blockers() {
        assert!(maker_backtest_gate_passes(&passing_evidence()));
    }

    #[test]
    fn failed_verdict_blocks_go_live() {
        let evidence = MakerBacktestEvidence {
            verdict: MakerBacktestVerdict::Fail,
            ..passing_evidence()
        };
        assert_eq!(
            maker_backtest_gate_blockers(&evidence),
            vec![MakerBacktestGateBlocker::VerdictNotPass]
        );
    }

    #[test]
    fn missing_build_and_strategy_identity_blocks_go_live() {
        let evidence = MakerBacktestEvidence {
            build_head_sha_valid: false,
            strategy_config_hash_valid: false,
            result_contract_strategy_config_hash_valid: false,
            ..passing_evidence()
        };
        let blockers = maker_backtest_gate_blockers(&evidence);
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingBuildHeadSha));
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingStrategyConfigHash));
        assert!(
            blockers.contains(&MakerBacktestGateBlocker::ResultContractStrategyConfigHashMismatch)
        );
    }

    #[test]
    fn missing_orders_and_passive_fills_block_go_live() {
        let evidence = MakerBacktestEvidence {
            maker_orders_observed: false,
            passive_fills_observed: false,
            ..passing_evidence()
        };
        let blockers = maker_backtest_gate_blockers(&evidence);
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingMakerOrders));
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingPassiveFills));
    }

    #[test]
    fn missing_artifact_digests_block_go_live() {
        let evidence = MakerBacktestEvidence {
            run_artifact_sha256_valid: false,
            threshold_artifact_sha256_valid: false,
            execution_model_artifact_sha256_valid: false,
            ..passing_evidence()
        };
        let blockers = maker_backtest_gate_blockers(&evidence);
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingRunArtifactDigest));
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingThresholdArtifactDigest));
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingExecutionModelArtifactDigest));
    }

    #[test]
    fn missing_queue_position_blocks_go_live() {
        let evidence = MakerBacktestEvidence {
            queue_position_enabled: false,
            ..passing_evidence()
        };
        assert!(
            maker_backtest_gate_blockers(&evidence)
                .contains(&MakerBacktestGateBlocker::QueuePositionDisabled)
        );
    }

    #[test]
    fn trade_and_book_corpus_are_both_required() {
        let evidence = MakerBacktestEvidence {
            trade_ticks_present: false,
            order_book_deltas_present: false,
            ..passing_evidence()
        };
        let blockers = maker_backtest_gate_blockers(&evidence);
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingTradeTicks));
        assert!(blockers.contains(&MakerBacktestGateBlocker::MissingOrderBookDeltas));
    }

    #[test]
    fn custom_fill_model_requires_source_proof() {
        let evidence = MakerBacktestEvidence {
            custom_fill_model_used: true,
            custom_fill_model_source_proven: false,
            ..passing_evidence()
        };
        assert!(
            maker_backtest_gate_blockers(&evidence)
                .contains(&MakerBacktestGateBlocker::CustomFillModelWithoutSourceProof)
        );
    }

    #[test]
    fn source_proven_custom_fill_does_not_block_by_itself() {
        let evidence = MakerBacktestEvidence {
            custom_fill_model_used: true,
            custom_fill_model_source_proven: true,
            ..passing_evidence()
        };
        assert!(maker_backtest_gate_passes(&evidence));
    }
}
