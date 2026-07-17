use anyhow::Result;

pub mod binary_oracle_edge_taker;
pub mod binary_oracle_maker;
pub mod complete_set_arbitrage;
pub mod registry;

use registry::StrategyRegistry;

pub fn production_strategy_registry() -> Result<StrategyRegistry> {
    let mut registry = StrategyRegistry::new();
    registry.register::<binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder>()?;
    registry.register::<complete_set_arbitrage::CompleteSetArbitrageBuilder>()?;
    registry.register::<binary_oracle_maker::BinaryOracleMakerBuilder>()?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_void_hook_uses_boundary(hooks: &str, expected_body: &str) -> bool {
        if hooks.contains("/*") {
            return false;
        }

        let code = hooks
            .lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(signature_start) = code.find("fn on_order_fill_voided") else {
            return false;
        };
        let function = &code[signature_start..];
        let Some(body_start) = function.find('{') else {
            return false;
        };
        let mut depth = 0_u32;
        let body_end = function[body_start..]
            .char_indices()
            .find_map(|(index, character)| match character {
                '{' => {
                    depth += 1;
                    None
                }
                '}' => {
                    depth = depth.checked_sub(1)?;
                    (depth == 0).then_some(body_start + index)
                }
                _ => None,
            });
        let Some(body_end) = body_end else {
            return false;
        };
        function[body_start + 1..body_end]
            .split_whitespace()
            .collect::<String>()
            == expected_body.split_whitespace().collect::<String>()
    }

    #[test]
    fn production_registry_contains_expected_archetypes() {
        let registry =
            production_strategy_registry().expect("production strategy registry should build");
        let kinds = registry.kinds();
        assert!(
            kinds.contains(&binary_oracle_edge_taker::KEY),
            "taker archetype must stay registered, got: {kinds:?}"
        );
        assert!(
            kinds.contains(&complete_set_arbitrage::KEY),
            "complete-set arbitrage archetype must stay registered, got: {kinds:?}"
        );
        assert!(
            kinds.contains(&binary_oracle_maker::KEY),
            "maker archetype must be registered, got: {kinds:?}"
        );
    }

    #[test]
    fn production_strategies_override_the_fill_voided_no_op() {
        let cases = [
            (
                binary_oracle_edge_taker::KEY,
                "nautilus_strategy!(BinaryOracleEdgeTaker, {",
                "crate::bolt_v3_order_execution::fail_closed_on_order_fill_voided(event);",
                include_str!("binary_oracle_edge_taker/mod.rs"),
            ),
            (
                binary_oracle_maker::KEY,
                "nautilus_strategy!(BinaryOracleMaker, {",
                "crate::bolt_v3_order_execution::fail_closed_on_order_fill_voided(event);",
                include_str!("binary_oracle_maker/mod.rs"),
            ),
            (
                complete_set_arbitrage::KEY,
                "nautilus_strategy!(CompleteSetArbitrage, {",
                "fail_closed_on_order_fill_voided(event);",
                include_str!("complete_set_arbitrage/mod.rs"),
            ),
        ];
        let guarded_kinds = cases
            .iter()
            .map(|(kind, _, _, _)| *kind)
            .collect::<std::collections::BTreeSet<_>>();
        let registered_kinds = production_strategy_registry()
            .expect("production strategy registry should build")
            .kinds()
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            guarded_kinds, registered_kinds,
            "fill-void guard must cover exactly the production strategy registry"
        );

        for (kind, macro_marker, expected_body, source) in cases {
            let (_, hook_source) = source
                .split_once(macro_marker)
                .expect("registered strategy must use its declared hook macro");
            let (hooks, _) = hook_source
                .split_once("});")
                .expect("registered strategy hook macro must terminate");
            assert!(
                fill_void_hook_uses_boundary(hooks, expected_body),
                "registered strategy {kind} must override NT's fill-voided no-op and call the shared boundary from that hook body"
            );
        }
    }

    #[test]
    fn fill_void_guard_rejects_comment_only_spoofs() {
        assert!(!fill_void_hook_uses_boundary(
            "// fn on_order_fill_voided() {\n// fail_closed_on_order_fill_voided(event)\n// }",
            "fail_closed_on_order_fill_voided(event);"
        ));
        assert!(!fill_void_hook_uses_boundary(
            "/* fn on_order_fill_voided() { fail_closed_on_order_fill_voided(event); } */",
            "fail_closed_on_order_fill_voided(event);"
        ));
    }
}
