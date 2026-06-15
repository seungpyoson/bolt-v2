use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, bail};
use nautilus_analysis::{analyzer::PortfolioAnalyzer, statistic::PortfolioStatistic};
use nautilus_model::position::Position;

use crate::run_manifest::{
    DOMAIN_METRIC_CLOSED_POSITION_RATIO, ManifestDomainMetricConfig, registered_domain_metrics,
};

pub type DomainStatistic = Arc<dyn PortfolioStatistic<Item = f64> + Send + Sync>;

#[derive(Debug)]
pub struct ClosedPositionRatio;

impl PortfolioStatistic for ClosedPositionRatio {
    type Item = f64;

    fn name(&self) -> String {
        DOMAIN_METRIC_CLOSED_POSITION_RATIO.to_string()
    }

    fn calculate_from_positions(&self, positions: &[Position]) -> Option<f64> {
        if positions.is_empty() {
            return Some(0.0);
        }
        let closed = positions
            .iter()
            .filter(|position| position.is_closed())
            .count();
        Some(closed as f64 / positions.len() as f64)
    }
}

pub fn resolve_domain_statistics(
    configs: &[ManifestDomainMetricConfig],
) -> Result<Vec<DomainStatistic>> {
    let mut statistics = Vec::with_capacity(configs.len());
    for config in configs {
        match config.kind.as_str() {
            DOMAIN_METRIC_CLOSED_POSITION_RATIO => {
                statistics.push(Arc::new(ClosedPositionRatio) as DomainStatistic);
            }
            other => bail!(
                "domain metric {other:?} is not registered; registered metrics: {:?}",
                registered_domain_metrics()
            ),
        }
    }
    Ok(statistics)
}

pub fn register_domain_statistics(
    analyzer: &mut PortfolioAnalyzer,
    statistics: &[DomainStatistic],
) {
    for statistic in statistics {
        analyzer.register_statistic(Arc::clone(statistic));
    }
}

pub fn domain_statistics_from_analyzer(
    analyzer: &PortfolioAnalyzer,
    statistics: &[DomainStatistic],
) -> BTreeMap<String, f64> {
    let general = analyzer.get_performance_stats_general();
    statistics
        .iter()
        .filter_map(|statistic| {
            let name = statistic.name();
            general.get(&name).map(|value| (name, *value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_domain_statistics_with_nt_portfolio_analyzer() {
        let statistics = resolve_domain_statistics(&[ManifestDomainMetricConfig {
            kind: DOMAIN_METRIC_CLOSED_POSITION_RATIO.to_string(),
        }])
        .expect("resolve metric");
        let mut analyzer = PortfolioAnalyzer::new();

        register_domain_statistics(&mut analyzer, &statistics);

        assert!(
            analyzer
                .statistic(DOMAIN_METRIC_CLOSED_POSITION_RATIO)
                .is_some()
        );
    }

    #[test]
    fn closed_position_ratio_reports_zero_without_positions() {
        let statistic = ClosedPositionRatio;

        assert_eq!(statistic.calculate_from_positions(&[]), Some(0.0));
    }
}
