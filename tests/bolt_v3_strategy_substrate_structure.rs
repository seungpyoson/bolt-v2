use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[path = "support/rust_source_tokens.rs"]
mod rust_source_tokens;

use rust_source_tokens::{Token, count_sequence, item_body_tokens, item_header, texts, tokenize};

const ARCHETYPES: &[&str] = &[
    "src/strategies/binary_oracle_edge_taker/archetype.rs",
    "src/strategies/binary_oracle_maker/archetype.rs",
    "src/strategies/complete_set_arbitrage/archetype.rs",
];

const STRATEGY_MUTATION_AUTHORITY_NAMES: &[&str] = &[
    "BoltV3NtSubmitOnlySink",
    "BoltV3NtVenueMutationSink",
    "BoltV3OrderExecutionMode",
    "BoltV3OrderExecutionPolicy",
    "NtStrategyVenueMutationSink",
    "with_order_execution_policy",
];

// Bounded interim evidence only: these names mirror the two retained Python fences, with
// `modify_order_via_nt` added to cover the existing Bolt sink trait. This is not an exhaustive
// census of every NautilusTrader API that could transitively expose mutable authority; structural
// unnameability is separate work under #1407.
const NT_VENUE_MUTATION_METHOD_NAMES: &[&str] = &[
    "core_mut",
    "order_manager",
    "send_risk_command",
    "send_exec_command",
    "send_emulator_command",
    "send_algo_command",
    "send_trading_command",
    "send_any",
    "send_any_value",
    "risk_engine_queue_execute",
    "exec_engine_queue_execute",
    "emulator_queue_execute",
    "algo_engine_queue_execute",
    "submit_order",
    "submit_order_list",
    "modify_order",
    "cancel_order",
    "cancel_orders",
    "cancel_all_orders",
    "close_position",
    "close_all_positions",
    "submit_order_via_nt",
    "cancel_order_via_nt",
    "cancel_all_orders_via_nt",
    "modify_order_via_nt",
    "submit_order_with_params",
    "submit_order_list_with_params",
    "modify_order_with_params",
    "cancel_order_with_params",
    "cancel_orders_with_params",
    "cancel_all_orders_with_params",
    "modify_order_in_place",
    "expire_gtd_order",
    "reactivate_gtd_timers",
    "set_gtd_expiry",
    "cancel_gtd_expiry",
    "finalize_market_exit",
    "cancel_market_exit",
    "deny_order",
    "deny_order_list",
    "market_exit_strategy",
    "exit_market",
    "market_exit",
];

const NT_VENUE_MUTATION_BARE_NAMES: &[&str] = &[
    "send_trading_command",
    "send_any",
    "send_any_value",
    "risk_engine_queue_execute",
    "exec_engine_queue_execute",
    "emulator_queue_execute",
    "algo_engine_queue_execute",
];

const NT_TRADING_COMMAND_SURFACE_NAMES: &[&str] = &[
    "TradingCommand",
    "SubmitOrder",
    "SubmitOrderList",
    "ModifyOrder",
    "CancelOrder",
    "CancelOrders",
    "CancelAllOrders",
    "ClosePosition",
    "CloseAllPositions",
    "DenyOrder",
    "DenyOrderList",
];

// `verify_bolt_v3_no_exit_market_command.py` reserves this exact token in every context. Keep its
// predicate separate from the narrower strategy-policy command-surface matcher above so the Rust
// evidence preserves the originating rule rather than only its vocabulary.
const NT_EXIT_MARKET_COMMAND_NAME: &str = "ExitMarket";

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn source_tokens(relative: &str) -> Vec<Token> {
    let source = std::fs::read_to_string(repo_path(relative))
        .unwrap_or_else(|error| panic!("{relative} should be readable: {error}"));
    tokenize(&source)
}

fn production_tokens(source: &str) -> Vec<Token> {
    let tokens = tokenize(source);
    let macro_token_tree = macro_token_tree_mask(&tokens);
    let mut production = Vec::with_capacity(tokens.len());
    let mut cursor = 0;
    while cursor < tokens.len() {
        if !macro_token_tree[cursor]
            && let Some(attribute_end) = cfg_test_attribute_end(&tokens, cursor)
        {
            cursor = cfg_gated_item_end(&tokens, attribute_end);
        } else {
            production.push(tokens[cursor].clone());
            cursor += 1;
        }
    }
    production
}

fn macro_token_tree_mask(tokens: &[Token]) -> Vec<bool> {
    let mut mask = vec![false; tokens.len()];
    let mut delimiters: Vec<(&str, bool)> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let inside_macro = delimiters.iter().any(|(_, is_macro)| *is_macro);
        mask[index] = inside_macro;
        match token.text.as_str() {
            "(" | "[" | "{" => {
                let close = match token.text.as_str() {
                    "(" => ")",
                    "[" => "]",
                    "{" => "}",
                    _ => unreachable!(),
                };
                let opens_macro = tokens
                    .get(index.wrapping_sub(1))
                    .is_some_and(|token| token.text == "!")
                    || (tokens
                        .get(index.wrapping_sub(2))
                        .is_some_and(|token| token.text == "!")
                        && tokens
                            .get(index.wrapping_sub(3))
                            .is_some_and(|token| token.text == "macro_rules"));
                delimiters.push((close, inside_macro || opens_macro));
            }
            ")" | "]" | "}" => {
                if delimiters
                    .last()
                    .is_some_and(|(close, _)| *close == token.text)
                {
                    delimiters.pop();
                }
            }
            _ => {}
        }
    }
    mask
}

fn cfg_test_attribute_end(tokens: &[Token], start: usize) -> Option<usize> {
    // Only an unconditional test build predicate is removed. Ambiguous expressions such as
    // `cfg(any(test, feature = ...))` remain in the production scan; a test-only compound cfg may
    // therefore over-match, but this retained safety fence must never hide production-capable code.
    const CFG_TEST_ATTRIBUTE: &[&str] = &["#", "[", "cfg", "(", "test", ")", "]"];
    (texts(tokens.get(start..start + CFG_TEST_ATTRIBUTE.len())?) == CFG_TEST_ATTRIBUTE)
        .then_some(start + CFG_TEST_ATTRIBUTE.len())
}

fn cfg_gated_item_end(tokens: &[Token], mut cursor: usize) -> usize {
    // Stop at the first complete construct boundary and never cross an enclosing brace. For a
    // structured match-arm pattern, the first balanced brace can precede the gated body; retaining
    // that body is an intentional fail-closed over-match rather than hiding a production sibling.
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    while cursor < tokens.len() {
        match tokens[cursor].text.as_str() {
            "(" => parentheses += 1,
            ")" => parentheses = parentheses.saturating_sub(1),
            "[" => brackets += 1,
            "]" => brackets = brackets.saturating_sub(1),
            "{" if parentheses == 0 && brackets == 0 => braces += 1,
            "}" if parentheses == 0 && brackets == 0 => {
                if braces == 0 {
                    return cursor;
                }
                braces -= 1;
                if braces == 0 {
                    return cursor + 1;
                }
            }
            ";" | "," if parentheses == 0 && brackets == 0 && braces == 0 => {
                return cursor + 1;
            }
            _ => {}
        }
        cursor += 1;
    }
    cursor
}

fn contains_sequence(tokens: &[Token], expected: &[&str]) -> bool {
    count_sequence(tokens, expected) > 0
}

fn references_crate_strategies(tokens: &[Token]) -> bool {
    if contains_sequence(tokens, &["crate", "::", "strategies"])
        || contains_sequence(tokens, &["super", "::", "strategies"])
    {
        return true;
    }
    let actual = texts(tokens);
    for start in 0..actual.len().saturating_sub(3) {
        if actual[start..start + 4] != ["use", "crate", "::", "{"] {
            continue;
        }
        let mut depth = 1usize;
        let mut cursor = start + 4;
        while cursor < actual.len() && depth > 0 {
            match actual[cursor] {
                "{" => depth += 1,
                "}" => depth -= 1,
                "strategies" => return true,
                _ => {}
            }
            cursor += 1;
        }
    }
    false
}

fn rust_files_below(root: &Path) -> Vec<PathBuf> {
    fn collect(root: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(root)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", root.display()))
        {
            let path = entry.expect("source entry should be readable").path();
            if path.is_dir() {
                if path.file_name().and_then(|name| name.to_str()) == Some("target") {
                    continue;
                }
                collect(&path, files);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    files
}

fn production_bolt_v3_files() -> Vec<PathBuf> {
    let src = repo_path("src");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&src).expect("src should be readable") {
        let path = entry.expect("src entry should be readable").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !name.starts_with("bolt_v3_") {
            continue;
        }
        if path.is_dir() {
            files.extend(rust_files_below(&path));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn production_strategy_files() -> Vec<PathBuf> {
    rust_files_below(&repo_path("src/strategies"))
        .into_iter()
        .filter(|path| {
            !path
                .components()
                .any(|component| component.as_os_str() == "tests")
        })
        .collect()
}

fn named_strategy_mutation_surfaces(tokens: &[Token]) -> Vec<&str> {
    let actual = texts(tokens);
    let mut violations: Vec<&str> = STRATEGY_MUTATION_AUTHORITY_NAMES
        .iter()
        .copied()
        .filter(|name| actual.contains(name))
        .collect();
    violations.extend(
        NT_TRADING_COMMAND_SURFACE_NAMES
            .iter()
            .copied()
            .filter(|name| command_surface_reference(&actual, name)),
    );
    if actual.contains(&NT_EXIT_MARKET_COMMAND_NAME) {
        violations.push(NT_EXIT_MARKET_COMMAND_NAME);
    }
    violations.extend(
        NT_VENUE_MUTATION_METHOD_NAMES
            .iter()
            .copied()
            .filter(|name| direct_method_reference(&actual, name)),
    );
    violations.extend(
        NT_VENUE_MUTATION_BARE_NAMES
            .iter()
            .copied()
            .filter(|name| bare_function_reference(&actual, name)),
    );
    violations.sort_unstable();
    violations.dedup();
    violations
}

fn direct_method_reference(tokens: &[&str], name: &str) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        *token == name && index > 0 && matches!(tokens[index - 1], "." | "::")
    })
}

fn command_surface_reference(tokens: &[&str], name: &str) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        *token == name
            && tokens
                .get(index + 1)
                .is_none_or(|next| matches!(*next, "::" | "<" | "(" | ";" | "," | ":" | ")"))
    })
}

fn bare_function_reference(tokens: &[&str], name: &str) -> bool {
    tokens.iter().enumerate().any(|(index, token)| {
        *token == name
            && tokens
                .get(index + 1)
                .is_some_and(|next| matches!(*next, "(" | "::" | "<" | ";" | "," | ")"))
    })
}

fn trait_method_names<'a>(tokens: &'a [Token], trait_name: &str) -> Vec<&'a str> {
    let actual = texts(tokens);
    let Some(trait_start) = actual
        .windows(2)
        .position(|window| window == ["trait", trait_name])
    else {
        return Vec::new();
    };
    let Some(body_start) = actual[trait_start + 2..]
        .iter()
        .position(|token| *token == "{")
        .map(|offset| trait_start + 2 + offset)
    else {
        return Vec::new();
    };
    let mut methods = Vec::new();
    let mut depth = 1usize;
    let mut cursor = body_start + 1;
    while cursor < actual.len() && depth != 0 {
        match actual[cursor] {
            "{" => depth += 1,
            "}" => depth -= 1,
            "fn" if depth == 1 => {
                if let Some(method) = actual.get(cursor + 1) {
                    methods.push(*method);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    methods
}

fn workspace_crate_files() -> Vec<PathBuf> {
    rust_files_below(&repo_path("crates"))
}

fn anonymous_use_group_open(actual: &[&str], index: usize) -> Option<usize> {
    let mut depth = 0usize;
    for cursor in (0..index).rev() {
        match actual[cursor] {
            "}" => depth += 1,
            "{" if depth > 0 => depth -= 1,
            "{" => {
                let anonymous_root = actual.get(cursor.wrapping_sub(1)) == Some(&"use")
                    || (actual.get(cursor.wrapping_sub(1)) == Some(&"::")
                        && actual.get(cursor.wrapping_sub(2)) == Some(&"use"));
                return anonymous_root.then_some(cursor);
            }
            _ => {}
        }
    }
    None
}

fn is_root_alias_binding(actual: &[&str], index: usize, aliases: &BTreeSet<String>) -> bool {
    if !actual
        .get(index)
        .is_some_and(|root| aliases.contains(*root))
        || actual.get(index + 1) != Some(&"as")
        || actual.get(index + 2) == Some(&"_")
    {
        return false;
    }
    match actual.get(index.wrapping_sub(1)) {
        Some(&"use") => true,
        Some(&"::") => actual.get(index.wrapping_sub(2)) == Some(&"use"),
        Some(&"{") | Some(&",") => anonymous_use_group_open(actual, index).is_some(),
        _ => false,
    }
}

fn bolt_v2_root_aliases(actual: &[&str]) -> BTreeSet<String> {
    let mut aliases = BTreeSet::from(["bolt_v2".to_string()]);
    loop {
        let previous_len = aliases.len();
        for index in 0..actual.len() {
            if is_root_alias_binding(actual, index, &aliases)
                && let Some(alias) = actual.get(index + 2)
            {
                aliases.insert((*alias).to_string());
            }
        }
        for window in actual.windows(5) {
            if window[0..2] == ["extern", "crate"]
                && aliases.contains(window[2])
                && window[3] == "as"
                && window[4] != "_"
            {
                aliases.insert(window[4].to_string());
            }
        }
        let mut grouped_aliases = Vec::new();
        for (open, root) in actual.iter().enumerate() {
            if !aliases.contains(*root) || actual.get(open + 1..open + 3) != Some(&["::", "{"]) {
                continue;
            }
            let mut depth = 1usize;
            let mut cursor = open + 3;
            while cursor < actual.len() && depth > 0 {
                match actual[cursor] {
                    "{" => depth += 1,
                    "}" => depth = depth.saturating_sub(1),
                    "self"
                        if depth == 1
                            && is_use_tree_segment_start(actual, cursor)
                            && actual.get(cursor + 1) == Some(&"as")
                            && actual.get(cursor + 2) != Some(&"_") =>
                    {
                        if let Some(alias) = actual.get(cursor + 2) {
                            grouped_aliases.push((*alias).to_string());
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
        }
        aliases.extend(grouped_aliases);
        if aliases.len() == previous_len {
            return aliases;
        }
    }
}

fn belongs_to_bolt_v2_path(actual: &[&str], index: usize, root_aliases: &BTreeSet<String>) -> bool {
    if index >= 2 && actual[index - 1] == "::" && root_aliases.contains(actual[index - 2]) {
        return true;
    }
    for open in 0..index.saturating_sub(2) {
        if !root_aliases.contains(actual[open])
            || actual.get(open + 1..open + 3) != Some(&["::", "{"])
        {
            continue;
        }
        let mut depth = 1usize;
        for token in &actual[open + 3..index] {
            match *token {
                "{" => depth += 1,
                "}" => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if depth == 1 && is_use_tree_segment_start(actual, index) {
            return true;
        }
    }
    false
}

fn publicly_reexports_bolt_v2_root(actual: &[&str], aliases: &BTreeSet<String>) -> bool {
    let direct = actual.windows(4).any(|window| {
        window[0..2] == ["pub", "use"]
            && aliases.contains(window[2])
            && matches!(window[3], ";" | "as")
    }) || actual.windows(5).any(|window| {
        window[0..3] == ["pub", "use", "::"] && aliases.contains(window[3]) && window[4] == ";"
    }) || actual.windows(5).any(|window| {
        window[0..3] == ["pub", "extern", "crate"]
            && aliases.contains(window[3])
            && matches!(window[4], ";" | "as")
    }) || actual.windows(6).any(|window| {
        window[0..3] == ["pub", "use", "{"]
            && aliases.contains(window[3])
            && window[4..6] == ["}", ";"]
    });
    let anonymous_group = actual.iter().enumerate().any(|(index, root)| {
        if !aliases.contains(*root) || !is_use_tree_segment_start(actual, index) {
            return false;
        }
        let Some(open) = anonymous_use_group_open(actual, index) else {
            return false;
        };
        (actual.get(open.wrapping_sub(2)..open) == Some(&["pub", "use"]))
            || (actual.get(open.wrapping_sub(3)..open) == Some(&["pub", "use", "::"]))
    });
    direct
        || anonymous_group
        || actual.iter().enumerate().any(|(start, token)| {
            if *token != "pub" || actual.get(start + 1) != Some(&"use") {
                return false;
            }
            let (root, open_brace) = if actual.get(start + 2) == Some(&"::") {
                (actual.get(start + 3), start + 5)
            } else {
                (actual.get(start + 2), start + 4)
            };
            root.is_some_and(|root| aliases.contains(*root))
                && actual.get(open_brace) == Some(&"{")
                && use_tree_contains_at_depth(actual, open_brace, "self")
        })
}

fn imports_bolt_v2_strategies_namespace(actual: &[&str], root_aliases: &BTreeSet<String>) -> bool {
    actual.iter().enumerate().any(|(index, token)| {
        if *token != "strategies" || !belongs_to_bolt_v2_path(actual, index, root_aliases) {
            return false;
        }
        match actual.get(index + 1) {
            None | Some(&";") | Some(&"as") | Some(&",") | Some(&"}") => true,
            Some(&"::") => match actual.get(index + 2) {
                Some(&"*") => true,
                Some(&"{") => {
                    use_tree_contains_at_depth(actual, index + 2, "self")
                        || use_tree_contains_at_depth(actual, index + 2, "*")
                }
                _ => false,
            },
            _ => false,
        }
    })
}

fn references_retired_registry_type(tokens: &[Token], type_name: &str) -> bool {
    let actual = texts(tokens);
    let root_aliases = bolt_v2_root_aliases(&actual);
    if publicly_reexports_bolt_v2_root(&actual, &root_aliases)
        || imports_bolt_v2_strategies_namespace(&actual, &root_aliases)
    {
        return true;
    }
    for start in 0..actual.len() {
        if actual.get(start..start + 3) == Some(&["strategies", "::", "registry"])
            && belongs_to_bolt_v2_path(&actual, start, &root_aliases)
        {
            if matches!(
                actual.get(start + 3),
                None | Some(&";") | Some(&",") | Some(&"}") | Some(&"as")
            ) {
                return true;
            }
            if actual.get(start + 3) != Some(&"::") {
                continue;
            }
            if matches!(actual.get(start + 4), Some(name) if *name == type_name || *name == "*") {
                return true;
            }
            if use_tree_contains_at_depth(&actual, start + 4, type_name) {
                return true;
            }
        }
        if actual.get(start..start + 3) == Some(&["strategies", "::", "{"])
            && belongs_to_bolt_v2_path(&actual, start, &root_aliases)
        {
            let mut depth = 1usize;
            let mut cursor = start + 3;
            while cursor < actual.len() && depth > 0 {
                match actual[cursor] {
                    "{" => depth += 1,
                    "}" => depth -= 1,
                    "registry" if depth == 1 && is_use_tree_segment_start(&actual, cursor) => {
                        if matches!(
                            actual.get(cursor + 1),
                            None | Some(&";") | Some(&",") | Some(&"}") | Some(&"as")
                        ) {
                            return true;
                        }
                        if actual.get(cursor + 1) != Some(&"::") {
                            cursor += 1;
                            continue;
                        }
                        if matches!(
                            actual.get(cursor + 2),
                            Some(name) if *name == type_name || *name == "*"
                        ) || use_tree_contains_at_depth(&actual, cursor + 2, type_name)
                        {
                            return true;
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
        }
    }
    false
}

fn use_tree_contains_at_depth(actual: &[&str], open_brace: usize, type_name: &str) -> bool {
    if actual.get(open_brace) != Some(&"{") {
        return false;
    }
    let mut depth = 1usize;
    let mut cursor = open_brace + 1;
    while cursor < actual.len() && depth > 0 {
        match actual[cursor] {
            "{" => depth += 1,
            "}" => depth -= 1,
            name if depth == 1
                && (name == type_name || name == "*" || name == "self")
                && is_use_tree_segment_start(actual, cursor) =>
            {
                return true;
            }
            _ => {}
        }
        cursor += 1;
    }
    false
}

fn is_use_tree_segment_start(actual: &[&str], index: usize) -> bool {
    matches!(actual.get(index.wrapping_sub(1)), Some(&"{") | Some(&","))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(repo_path(""))
        .expect("source path should be under repository root")
        .to_string_lossy()
        .replace('\\', "/")
}

fn public_item_kind<'a>(tokens: &'a [Token], start: usize) -> Option<&'a str> {
    let mut cursor = start + 1;
    if tokens.get(cursor).is_some_and(|token| token.text == "(") {
        let mut depth = 1usize;
        cursor += 1;
        while cursor < tokens.len() && depth > 0 {
            match tokens[cursor].text.as_str() {
                "(" => depth += 1,
                ")" => depth = depth.saturating_sub(1),
                _ => {}
            }
            cursor += 1;
        }
    }
    while tokens.get(cursor).is_some_and(|token| {
        matches!(
            token.text.as_str(),
            "async" | "auto" | "const" | "default" | "extern" | "unsafe"
        )
    }) {
        cursor += 1;
    }
    tokens.get(cursor).map(|token| token.text.as_str())
}

fn public_declarations(tokens: &[Token]) -> Vec<Vec<&str>> {
    let mut declarations = Vec::new();
    for (start, token) in tokens.iter().enumerate() {
        if token.text != "pub" {
            continue;
        }
        let item_kind = public_item_kind(tokens, start);
        let include_body = matches!(item_kind, Some("enum" | "trait"));
        let mut declaration = Vec::new();
        let mut cursor = start;
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut angles = 0usize;
        let mut braces = 0usize;
        while let Some(current) = tokens.get(cursor) {
            if item_kind == Some("trait") && braces == 1 && current.text == "{" {
                let mut implementation_depth = 1usize;
                cursor += 1;
                while let Some(implementation) = tokens.get(cursor) {
                    match implementation.text.as_str() {
                        "{" => implementation_depth += 1,
                        "}" => {
                            implementation_depth = implementation_depth.saturating_sub(1);
                            if implementation_depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    cursor += 1;
                }
                cursor += 1;
                continue;
            }
            declaration.push(current.text.as_str());
            match current.text.as_str() {
                "(" if item_kind == Some("struct")
                    && parentheses == 0
                    && brackets == 0
                    && angles == 0 =>
                {
                    break;
                }
                "(" => parentheses += 1,
                ")" => parentheses = parentheses.saturating_sub(1),
                "[" => brackets += 1,
                "]" => brackets = brackets.saturating_sub(1),
                "<" => angles += 1,
                ">" => angles = angles.saturating_sub(1),
                "{" if parentheses == 0 && brackets == 0 && angles == 0 => {
                    if !include_body {
                        break;
                    }
                    braces += 1;
                }
                "{" if braces > 0 => braces += 1,
                "}" if braces > 0 => {
                    braces = braces.saturating_sub(1);
                    if braces == 0 {
                        break;
                    }
                }
                ";" | "," if parentheses == 0 && brackets == 0 && angles == 0 && braces == 0 => {
                    break;
                }
                _ => {}
            }
            cursor += 1;
        }
        declarations.push(declaration);
    }
    declarations
}

fn public_declaration_tokens(tokens: &[Token]) -> Vec<&str> {
    public_declarations(tokens).into_iter().flatten().collect()
}

fn normalized_public_declarations(tokens: &[Token]) -> Vec<String> {
    public_declarations(tokens)
        .into_iter()
        .map(|declaration| declaration.join(" "))
        .collect()
}

fn brace_opens_inline_module(tokens: &[Token], brace: usize) -> bool {
    let header_start = (0..brace)
        .rev()
        .find(|index| matches!(tokens[*index].text.as_str(), ";" | "{" | "}"))
        .map_or(0, |index| index + 1);
    tokens[header_start..brace]
        .windows(2)
        .any(|window| window[0].text == "mod" && window[1].text != "!")
}

fn forbidden_aliases_across_sources(
    sources: &[Vec<Token>],
    forbidden: &[&str],
) -> BTreeMap<String, String> {
    let mut aliases = forbidden
        .iter()
        .map(|name| ((*name).to_string(), (*name).to_string()))
        .collect::<BTreeMap<_, _>>();

    loop {
        let mut discovered = Vec::new();
        for tokens in sources {
            for window in tokens.windows(3) {
                if window[1].text == "as"
                    && window[2].text != "_"
                    && let Some(root) = aliases.get(&window[0].text)
                {
                    discovered.push((window[2].text.clone(), root.clone()));
                }
            }
            let mut scopes_are_modules = Vec::new();
            for (index, token) in tokens.iter().enumerate() {
                match token.text.as_str() {
                    "{" => {
                        scopes_are_modules.push(brace_opens_inline_module(tokens, index));
                        continue;
                    }
                    "}" => {
                        scopes_are_modules.pop();
                        continue;
                    }
                    _ => {}
                }
                if token.text != "type" {
                    continue;
                }
                // A source file and its inline `mod` bodies are module scopes.
                // Associated types and function-local aliases are not aliases
                // in the public item's module namespace.
                if scopes_are_modules.iter().any(|is_module| !is_module) {
                    continue;
                }
                let Some(alias) = tokens.get(index + 1) else {
                    continue;
                };
                let Some(statement) = tokens.get(index + 2..) else {
                    continue;
                };
                let Some(end_offset) = statement.iter().position(|token| token.text == ";") else {
                    continue;
                };
                let statement = &statement[..end_offset];
                let Some(equals) = statement.iter().position(|token| token.text == "=") else {
                    continue;
                };
                if let Some(root) = statement[equals + 1..]
                    .iter()
                    .find_map(|token| aliases.get(&token.text))
                {
                    discovered.push((alias.text.clone(), root.clone()));
                }
            }
        }
        let previous_len = aliases.len();
        aliases.extend(discovered);
        if aliases.len() == previous_len {
            return aliases;
        }
    }
}

#[test]
fn tokenizer_ignores_comments_strings_raw_strings_chars_and_lifetimes() {
    let source = r###"
        // crate::strategies::fake::KEY clients.get resolve_fee_provider
        /* nested /* crate::strategies::fake */ clients.get */
        const TEXT: &str = "crate::strategies::fake::KEY clients.get";
        const RAW: &str = r#"resolve_fee_provider crate::strategies"#;
        const CH: char = 'g';
        fn visible<'a>(value: &'a str) { assemble_strategy_build_context(value); }
        "###;
    let controls = tokenize(source);
    assert_eq!(count_sequence(&controls, &["crate", "::", "strategies"]), 0);
    assert_eq!(count_sequence(&controls, &["clients", ".", "get"]), 0);
    assert_eq!(count_sequence(&controls, &["resolve_fee_provider"]), 0);
    assert_eq!(
        count_sequence(&controls, &["assemble_strategy_build_context"]),
        1
    );
    let visible_body = item_body_tokens(&controls, &["fn", "visible"])
        .expect("the real function body should be located through tokens");
    assert_eq!(
        count_sequence(visible_body, &["assemble_strategy_build_context", "("]),
        1
    );
    let visible_header = item_header(source, &controls, &["fn", "visible"])
        .expect("the real function header should be located through tokens");
    assert!(visible_header.contains("visible<'a>"));
}

#[test]
fn retired_registry_matcher_covers_direct_and_grouped_paths_only() {
    for source in [
        "use bolt_v2::strategies::registry::FeeProvider;",
        "use bolt_v2::strategies::registry::{FeeProvider, StrategyBuilder};",
        "use bolt_v2::strategies::{registry::FeeProvider, production_strategy_registry};",
        "use bolt_v2::strategies::{registry::{FeeProvider, StrategyBuildContext}};",
    ] {
        assert!(references_retired_registry_type(
            &tokenize(source),
            "FeeProvider"
        ));
    }
    let unrelated = tokenize(
        "use other::registry::{FeeProvider}; use bolt_v2::strategies::production_strategy_registry; use bolt_v2::strategies::registry::{nested::FeeProvider}; use bolt_v2::strategies::{nested::{registry::FeeProvider}};",
    );
    assert!(!references_retired_registry_type(&unrelated, "FeeProvider"));
}

#[test]
fn retired_registry_matcher_covers_wildcard_and_reexport_paths() {
    for source in [
        "use bolt_v2::strategies::registry::*;",
        "use bolt_v2::strategies::{registry::*};",
        "pub use bolt_v2::strategies::registry::FeeProvider;",
        "pub use bolt_v2::strategies::registry::FeeProvider as SharedFeeProvider;",
        "pub use bolt_v2::strategies::registry::*;",
        "pub use bolt_v2::strategies::registry as retired_registry;",
        "pub use bolt_v2::strategies::{registry as retired_registry};",
        "use bolt_v2 as b; use b::strategies::registry::FeeProvider;",
        "use {bolt_v2 as b}; use b::strategies::registry::FeeProvider;",
        "extern crate bolt_v2 as b; use b::strategies::registry::*;",
        "pub use bolt_v2;",
        "pub use ::bolt_v2;",
        "pub use {bolt_v2};",
        "pub use {other, bolt_v2};",
        "use bolt_v2 as b; pub use {other, b};",
        "pub use bolt_v2::{self};",
        "pub use bolt_v2::{self as b};",
        "pub use ::bolt_v2::{self as b};",
        "pub use bolt_v2::strategies as shared_strategies;",
        "use bolt_v2::{strategies::*};",
        "use bolt_v2::{self as b}; use b::strategies::registry::FeeProvider;",
        "use bolt_v2::{self as b, self as c}; use c::strategies::registry::FeeProvider;",
    ] {
        assert!(
            references_retired_registry_type(&tokenize(source), "FeeProvider"),
            "missed retired registry import: {source}"
        );
    }
    for source in [
        "use other::strategies::registry::*;",
        "use other::bolt_v2 as b; use b::strategies::registry::FeeProvider;",
        "use bolt_v2 as b; use b::unrelated::FeeProvider;",
        "pub use bolt_v2::strategies::registry::nested::*;",
        "pub use bolt_v2::strategies::{nested::registry};",
    ] {
        assert!(
            !references_retired_registry_type(&tokenize(source), "FeeProvider"),
            "unrelated import was rejected: {source}"
        );
    }
}

#[test]
fn strategy_bindings_exports_only_the_two_neutral_production_lists() {
    let source = std::fs::read_to_string(repo_path("src/strategy_bindings.rs"))
        .expect("strategy bindings should be readable");
    let tokens = production_tokens(&source);
    assert_eq!(
        normalized_public_declarations(&tokens),
        vec![
            "pub fn production_runtime_bindings ( ) - > & [ StrategyRuntimeBinding ] {".to_string(),
            "pub fn production_validation_bindings ( ) - > & [ ArchetypeValidationBinding ] {"
                .to_string(),
        ],
        "strategy_bindings must expose exactly the two neutral production lists"
    );

    let imports: &[&[&str]] = &[
        &[
            "use",
            "crate",
            "::",
            "bolt_v3_archetypes",
            "::",
            "ArchetypeValidationBinding",
            ";",
        ],
        &[
            "use",
            "crate",
            "::",
            "bolt_v3_strategy_registration",
            "::",
            "StrategyRuntimeBinding",
            ";",
        ],
        &[
            "use",
            "crate",
            "::",
            "strategies",
            "::",
            "{",
            "binary_oracle_edge_taker",
            ",",
            "binary_oracle_maker",
            ",",
            "complete_set_arbitrage",
            "}",
            ";",
        ],
    ];
    for exact in imports {
        assert_eq!(
            count_sequence(&tokens, exact),
            1,
            "strategy_bindings import provenance differs: {}",
            exact.join("")
        );
    }
    let actual = texts(&tokens);
    assert_eq!(
        actual.iter().filter(|token| **token == "use").count(),
        imports.len(),
        "strategy_bindings must not add an attributed or alternate import"
    );
    for forbidden in ["#", "!", "include", "macro", "macro_rules", "macro_export"] {
        assert!(
            !actual.contains(&forbidden),
            "strategy_bindings production surface contains `{forbidden}`"
        );
    }
}

#[test]
fn public_api_forbidden_type_aliases_cannot_launder_handle_types() {
    let sources = vec![
        tokenize("pub type NeutralHandle = LiveNodeHandle;"),
        tokenize("type SecondAlias = NeutralHandle;"),
        tokenize("pub fn leak() -> SecondAlias { loop {} }"),
        tokenize(
            "pub mod nested { type NestedAlias = LiveNodeHandle; pub fn leak() -> NestedAlias { loop {} } }",
        ),
    ];
    let aliases = forbidden_aliases_across_sources(&sources, &["LiveNodeHandle"]);
    assert_eq!(
        aliases.get("NeutralHandle").map(String::as_str),
        Some("LiveNodeHandle")
    );
    assert_eq!(
        aliases.get("SecondAlias").map(String::as_str),
        Some("LiveNodeHandle")
    );
    assert_eq!(
        aliases.get("NestedAlias").map(String::as_str),
        Some("LiveNodeHandle")
    );
    let public = public_declaration_tokens(&sources[2]);
    assert!(
        aliases.keys().any(|alias| public.contains(&alias.as_str())),
        "transitive forbidden alias should remain visible to the public-API scan"
    );
    let nested_public = public_declaration_tokens(&sources[3]);
    assert!(
        nested_public.contains(&"NestedAlias"),
        "inline-module aliases must remain visible to the public-API scan"
    );
}

#[test]
fn associated_types_are_not_treated_as_module_aliases() {
    let sources = vec![
        tokenize("impl StrategyBuilder for Builder { type Strategy = BinaryOracleEdgeTaker; }"),
        tokenize("pub fn route<S: Strategy>() {}"),
    ];
    let aliases = forbidden_aliases_across_sources(&sources, &["BinaryOracleEdgeTaker"]);
    assert_eq!(aliases.get("Strategy"), None);
}

#[test]
fn public_api_scan_includes_enum_variants_and_trait_signatures() {
    let tokens = tokenize(
        "pub enum LeakingEnum { Handle(LiveNodeHandle) }\n\
         pub trait LeakingTrait {\n\
             fn handle(&self) -> LiveNodeHandle;\n\
             fn internal_only(&self) -> bool {\n\
                 let _handle: PrivateImplementationHandle;\n\
                 true\n\
             }\n\
         }\n\
         pub struct NamedFields { pub handle: LiveNodeHandle, private: PrivateNamedHandle }\n\
         pub struct TupleFields(pub LiveNodeHandle, PrivateTupleHandle);\n\
         pub union UnionFields { pub handle: LiveNodeHandle, private: PrivateUnionHandle }",
    );
    let public = public_declaration_tokens(&tokens);
    assert!(
        public.contains(&"LiveNodeHandle"),
        "public enum variants, trait signatures, and public fields must be part of the public-API scan"
    );
    for private in [
        "PrivateImplementationHandle",
        "PrivateNamedHandle",
        "PrivateTupleHandle",
        "PrivateUnionHandle",
    ] {
        assert!(
            !public.contains(&private),
            "private implementation and field types are not part of the public API: {private}"
        );
    }
}

#[test]
fn production_tokenizer_handles_current_strategy_cfg_shapes() {
    let tokens = production_tokens(
        r#"
        fn production() { emit_order_intent(); }
        struct StrategyState {
            #[cfg(test)]
            test_only_field: SubmitOrder,
            production_field: ProductionField,
        }
        fn build_state() -> StrategyState {
            StrategyState {
                #[cfg(test)]
                test_only_field: SubmitOrder::default(),
                production_field: ProductionField::default(),
            }
        }
        #[cfg(test)]
        mod tests {
            fn fixture() { self.submit_order(order); }
        }
        #[cfg(test)]
        fixture! { self.cancel_all_orders(); }
        fn after_cfg_macro() { production_after_cfg_macro(); }
        #[cfg(test)]
        if test_mode() { self.modify_order(order); }
        fn after_cfg_expression() { production_after_cfg_expression(); }
        passthrough! { #[cfg(test)] }
        fn after_macro_tokens() { self.modify_order_via_nt(order); }
        match state {
            #[cfg(test)]
            State::Fixture { value } => { test_only_arm_body(value); }
            State::Production => production_after_gated_arm(),
        }
        #[cfg(not(test))]
        fn retained() { self.cancel_order(order_id); }
        "#,
    );
    assert_eq!(count_sequence(&tokens, &["emit_order_intent", "("]), 1);
    assert_eq!(count_sequence(&tokens, &["submit_order", "("]), 0);
    assert_eq!(count_sequence(&tokens, &["cancel_all_orders", "("]), 0);
    assert_eq!(count_sequence(&tokens, &["modify_order", "("]), 0);
    assert_eq!(count_sequence(&tokens, &["SubmitOrder"]), 0);
    assert_eq!(count_sequence(&tokens, &["ProductionField"]), 2);
    assert_eq!(count_sequence(&tokens, &["modify_order_via_nt", "("]), 1);
    assert_eq!(count_sequence(&tokens, &["cancel_order", "("]), 1);
    assert_eq!(count_sequence(&tokens, &["test_only_arm_body", "("]), 1);
    assert_eq!(
        count_sequence(&tokens, &["production_after_gated_arm", "("]),
        1
    );
    assert_eq!(
        count_sequence(&tokens, &["production_after_cfg_macro", "("]),
        1
    );
    assert_eq!(
        count_sequence(&tokens, &["production_after_cfg_expression", "("]),
        1
    );
}

#[test]
fn production_tokenizer_keeps_code_after_real_test_only_fields() {
    let tokens = production_tokens(
        &std::fs::read_to_string(repo_path("src/strategies/binary_oracle_edge_taker/mod.rs"))
            .expect("edge-taker strategy source should be readable"),
    );
    for production_name in [
        "SettlementEvidenceComputation",
        "apply_selection_snapshot",
        "warmup_tick_count",
    ] {
        assert!(
            contains_sequence(&tokens, &[production_name]),
            "production tokenizer must not hide `{production_name}` after a test-only field"
        );
    }
}

#[test]
fn bounded_strategy_mutation_surface_matcher_has_controls() {
    for authority in STRATEGY_MUTATION_AUTHORITY_NAMES {
        let tokens = tokenize(&format!("use boundary::{authority};"));
        assert_eq!(named_strategy_mutation_surfaces(&tokens), vec![*authority]);
    }
    for method in NT_VENUE_MUTATION_METHOD_NAMES {
        let tokens = tokenize(&format!("self.{method}(command);"));
        assert!(
            named_strategy_mutation_surfaces(&tokens).contains(method),
            "direct method matcher must retain {method}"
        );
    }
    let function_references = tokenize(
        "let exit = Self::exit_market as fn(&mut Self); let markets = [Self::market_exit]; let close = &self.close_position;",
    );
    for method in ["exit_market", "market_exit", "close_position"] {
        assert!(
            named_strategy_mutation_surfaces(&function_references).contains(&method),
            "function-reference matcher must retain {method}"
        );
    }
    for function in NT_VENUE_MUTATION_BARE_NAMES {
        let tokens = tokenize(&format!("{function}(command);"));
        assert!(
            named_strategy_mutation_surfaces(&tokens).contains(function),
            "bare transport matcher must retain {function}"
        );
    }
    for command in NT_TRADING_COMMAND_SURFACE_NAMES {
        let tokens = tokenize(&format!("use nt::{command};"));
        assert!(
            named_strategy_mutation_surfaces(&tokens).contains(command),
            "command-surface matcher must retain {command}"
        );
    }
    for source in [
        "use nautilus_model::{ExitMarket};",
        "match command { TradingCommand::ExitMarket => emit_order_intent() }",
        "match command { StrategyCommand::ExitMarket { instrument_id } => emit_order_intent() }",
    ] {
        assert!(
            named_strategy_mutation_surfaces(&tokenize(source)).contains(&"ExitMarket"),
            "the retained exact-token ExitMarket predicate must reject `{source}`"
        );
    }
    assert!(named_strategy_mutation_surfaces(&tokenize("emit_order_intent();")).is_empty());
    assert!(
        named_strategy_mutation_surfaces(&tokenize("submit_order_intent();")).is_empty(),
        "near-neighbor intent helper must not trip the exact mutation fence"
    );
    assert!(
        named_strategy_mutation_surfaces(&tokenize("emit_exit_market_intent(ExitMarketIntent);"))
            .is_empty(),
        "near-neighbor ExitMarket identifiers must remain allowed"
    );
    assert!(
        named_strategy_mutation_surfaces(&tokenize(
            "match intent { ExitAction::CancelOrder => emit_order_intent() }"
        ))
        .is_empty(),
        "Bolt-local intent variants must not be mistaken for NT command surfaces"
    );
}

#[test]
fn mutation_census_covers_every_nt_venue_sink_method() {
    let tokens = source_tokens("src/bolt_v3_order_execution.rs");
    let methods = trait_method_names(&tokens, "BoltV3NtVenueMutationSink");
    assert_eq!(
        methods,
        vec![
            "submit_order_via_nt",
            "cancel_order_via_nt",
            "cancel_all_orders_via_nt",
            "modify_order_via_nt",
        ]
    );
    for method in methods {
        assert!(
            NT_VENUE_MUTATION_METHOD_NAMES.contains(&method),
            "NT venue sink method `{method}` must remain in the retained mutation census"
        );
    }
}

#[test]
fn every_archetype_uses_the_shared_build_context_without_inlining_provider_lookup() {
    for relative in ARCHETYPES {
        let tokens = source_tokens(relative);
        assert_eq!(
            count_sequence(&tokens, &["assemble_strategy_build_context", "("]),
            1,
            "{relative} must call shared context assembly exactly once"
        );
        assert_eq!(
            count_sequence(&tokens, &["resolve_fee_provider"]),
            0,
            "{relative} must not inline fee-provider resolution"
        );
        assert_eq!(
            count_sequence(&tokens, &["clients", ".", "get"]),
            0,
            "{relative} must not reach into the client map"
        );
    }
}

#[test]
fn production_archetypes_use_the_neutral_venue_accessor() {
    let registration = source_tokens("src/bolt_v3_strategy_registration.rs");
    let accessor = item_body_tokens(&registration, &["fn", "venue_for_client"])
        .expect("neutral venue accessor should exist");
    assert_eq!(
        count_sequence(accessor, &["root", ".", "clients", ".", "get", "("]),
        1,
        "venue_for_client must remain the single direct venue lookup"
    );

    let mut production_calls = 0usize;
    for relative in ARCHETYPES {
        let source = std::fs::read_to_string(repo_path(relative))
            .expect("production archetype should be readable");
        let tokens = production_tokens(&source);
        assert_eq!(
            count_sequence(&tokens, &["clients", ".", "get", "("]),
            0,
            "{relative} must not bypass venue_for_client"
        );
        production_calls += count_sequence(&tokens, &["venue_for_client", "("]);
    }
    assert!(
        production_calls > 0,
        "production archetypes must positively use venue_for_client"
    );
}

// Secondary defense in depth. The authoritative dependency-direction policy is
// scripts/verify_bolt_v3_dependency_direction.py, which resolves Rust paths.
#[test]
fn production_bolt_v3_modules_do_not_reference_the_strategy_layer() {
    let mut violations = Vec::new();
    for path in production_bolt_v3_files() {
        let source = std::fs::read_to_string(&path).expect("production source should be readable");
        let tokens = tokenize(&source);
        if references_crate_strategies(&tokens) {
            violations.push(relative(&path));
        }
    }
    assert!(
        violations.is_empty(),
        "production bolt_v3 modules reference strategy paths: {violations:?}"
    );
}

#[test]
fn production_strategy_modules_do_not_name_nt_mutation_authority() {
    let mut violations = Vec::new();
    let strategy_files = production_strategy_files();
    assert!(
        !strategy_files.is_empty(),
        "strategy mutation fence must scan at least one production source"
    );
    for path in strategy_files {
        let source = std::fs::read_to_string(&path).expect("strategy source should be readable");
        let tokens = production_tokens(&source);
        for surface in named_strategy_mutation_surfaces(&tokens) {
            violations.push(format!("{}: {surface}", relative(&path)));
        }
    }
    assert!(
        violations.is_empty(),
        "production strategy modules name NT mutation authority outside shared admission: {violations:?}"
    );
}

#[test]
fn archetype_and_obsolete_full_path_layout_stays_retired() {
    let mut entries: Vec<String> = std::fs::read_dir(repo_path("src/bolt_v3_archetypes"))
        .expect("archetype directory should be readable")
        .map(|entry| {
            entry
                .expect("archetype entry should be readable")
                .file_name()
                .into_string()
                .expect("archetype file name should be UTF-8")
        })
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["mod.rs"]);

    let obsolete_paths: &[&[&str]] = &[
        &[
            "crate",
            "::",
            "bolt_v3_archetypes",
            "::",
            "binary_oracle_edge_taker",
        ],
        &[
            "crate",
            "::",
            "bolt_v3_archetypes",
            "::",
            "binary_oracle_maker",
        ],
        &["crate", "::", "bolt_v3_maker_settlement"],
        &["crate", "::", "bolt_v3_maker_runtime_settlement"],
        &[
            "crate",
            "::",
            "strategies",
            "::",
            "binary_oracle_edge_taker",
            "::",
            "settlement",
        ],
        &[
            "crate",
            "::",
            "strategies",
            "::",
            "binary_oracle_edge_taker",
            "::",
            "settlement_booking",
        ],
        &[
            "crate",
            "::",
            "strategies",
            "::",
            "complete_set_arbitrage",
            "::",
            "settlement",
        ],
        &[
            "crate",
            "::",
            "strategies",
            "::",
            "complete_set_arbitrage",
            "::",
            "settlement_booking",
        ],
        &["mod", "bolt_v3_maker_settlement", ";"],
        &["mod", "bolt_v3_maker_runtime_settlement", ";"],
    ];
    let mut violations = Vec::new();
    for path in rust_files_below(&repo_path("src")) {
        let source = std::fs::read_to_string(&path).expect("source should be readable");
        let tokens = tokenize(&source);
        for obsolete in obsolete_paths {
            if contains_sequence(&tokens, obsolete) {
                violations.push(format!("{}: {}", relative(&path), obsolete.join("")));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "obsolete full paths remain: {violations:?}"
    );
}

#[test]
fn workspace_crates_do_not_import_types_from_their_retired_registry_home() {
    let mut violations = Vec::new();
    for path in workspace_crate_files() {
        let source = std::fs::read_to_string(&path).expect("workspace source should be readable");
        let tokens = tokenize(&source);
        for type_name in ["FeeProvider", "StrategyBuildContext"] {
            if references_retired_registry_type(&tokens, type_name) {
                violations.push(format!(
                    "{}: strategies::registry::{type_name}",
                    relative(&path)
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "workspace crates import shared types from retired registry homes: {violations:?}"
    );
}

#[test]
fn live_node_does_not_depend_on_the_edge_taker_key() {
    for path in std::iter::once(repo_path("src/bolt_v3_live_node.rs"))
        .chain(rust_files_below(&repo_path("src/bolt_v3_live_node")))
    {
        let source = std::fs::read_to_string(&path).expect("live-node source should be readable");
        let tokens = tokenize(&source);
        assert!(
            !contains_sequence(&tokens, &["binary_oracle_edge_taker", "::", "KEY"]),
            "{} must resolve strategies through registered bindings",
            relative(&path)
        );
    }
}

#[test]
fn shared_runtime_public_apis_expose_no_taker_private_or_nt_handle_types() {
    let forbidden = [
        "BinaryOracleEdgeTaker",
        "OpenPositionState",
        "PendingEntryState",
        "PendingExitState",
        "ManagedPositionState",
        "ExposureState",
        "DataActor",
        "LiveNodeHandle",
        "CacheHandle",
        "ActorHandle",
        "OrderCache",
        "PositionCache",
    ];
    let crate_sources = rust_files_below(&repo_path("src"))
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(path).expect("crate source should be readable");
            production_tokens(&source)
        })
        .collect::<Vec<_>>();
    let aliases = forbidden_aliases_across_sources(&crate_sources, &forbidden);
    let mut scanned = Vec::new();
    for path in production_bolt_v3_files() {
        let relative = relative(&path);
        if relative == "src/bolt_v3_live_node.rs" || relative.starts_with("src/bolt_v3_live_node/")
        {
            continue;
        }
        let source =
            std::fs::read_to_string(&path).expect("shared runtime source should be readable");
        let tokens = production_tokens(&source);
        let public = public_declaration_tokens(&tokens);
        for (alias, root) in &aliases {
            assert!(
                !public.contains(&alias.as_str()),
                "{relative} public API exposes forbidden private/handle type `{root}` through `{alias}`"
            );
        }
        scanned.push(relative);
    }
    assert!(
        scanned.contains(&"src/bolt_v3_submit_admission.rs".to_string()),
        "shared public-API scan must cover submit admission"
    );
}

#[test]
fn every_archetype_has_one_capability_declaration_and_preparation_path() {
    for relative in ARCHETYPES {
        let tokens = source_tokens(relative);
        assert_eq!(
            count_sequence(
                &tokens,
                &["capabilities", ":", "StrategyRuntimeCapabilities", "{"]
            ),
            1,
            "{relative} must declare one capability set"
        );
        assert_eq!(
            count_sequence(&tokens, &["prepare", ":", "prepare_runtime_strategy"]),
            1,
            "{relative} must declare one preparation callback"
        );
        assert_eq!(
            count_sequence(&tokens, &["fn", "prepare_runtime_strategy", "("]),
            1,
            "{relative} must define one preparation entry point"
        );
        assert_eq!(
            count_sequence(&tokens, &[".", "prepare_strategy", "("]),
            1,
            "{relative} must have one registry preparation path"
        );
    }
}

#[test]
fn dependency_allowance_is_empty_and_shrink_only_gate_remains_wired() {
    let dependency_fence =
        std::fs::read_to_string(repo_path("scripts/verify_bolt_v3_dependency_direction.py"))
            .expect("dependency fence should be readable");
    assert!(
        dependency_fence
            .lines()
            .any(|line| { line.trim() == "FINDING_ALLOWANCES: tuple[FindingAllowance, ...] = ()" })
    );
    let runner = std::fs::read_to_string(repo_path("scripts/run_fences.py"))
        .expect("source-fence runner should be readable");
    assert!(runner.contains("scripts_dir.glob(\"verify_*.py\")"));
    assert!(dependency_fence.contains("enforce_shrink_only = argv is None"));
    assert!(dependency_fence.contains("return check_allowlist_shrink_only()"));
    assert!(dependency_fence.contains("The enforced `FINDING_ALLOWANCES` set is empty"));
}
