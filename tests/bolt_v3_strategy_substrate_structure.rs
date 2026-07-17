use std::path::{Path, PathBuf};

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

const NT_VENUE_MUTATION_METHOD_NAMES: &[&str] = &[
    "core_mut",
    "order_manager",
    "submit_order",
    "submit_order_list",
    "modify_order",
    "modify_orders",
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

const NT_TRANSITIVE_MUTATION_METHOD_NAMES: &[&str] = &[
    "strategy_core_mut",
    "reset_market_exit_state",
    "on_start",
    "on_time_event",
    "check_market_exit",
    "stop",
];

const NT_VENUE_MUTATION_BARE_NAMES: &[&str] = &[
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
];

const NT_TRADING_COMMAND_SURFACE_NAMES: &[&str] = &[
    "TradingCommand",
    "SubmitOrder",
    "SubmitOrderList",
    "ModifyOrder",
    "ModifyOrders",
    "BatchModifyOrders",
    "BatchCancelOrders",
    "CancelOrder",
    "CancelOrders",
    "CancelAllOrders",
    "ClosePosition",
    "CloseAllPositions",
    "DenyOrder",
    "DenyOrderList",
    "ExitMarket",
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct Token {
    text: String,
    is_raw_identifier: bool,
}

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn source_tokens(relative: &str) -> Vec<Token> {
    let source = std::fs::read_to_string(repo_path(relative))
        .unwrap_or_else(|error| panic!("{relative} should be readable: {error}"));
    tokenize(&source)
}

fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(b"//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index..].starts_with(b"/*") {
            index = scan_nested_block_comment(bytes, index);
            continue;
        }
        if let Some(end) = scan_raw_string(bytes, index) {
            index = end;
            continue;
        }
        if let Some(end) = scan_quoted_literal(bytes, index) {
            index = end;
            continue;
        }
        if let Some(end) = scan_char_or_lifetime(bytes, index) {
            index = end;
            continue;
        }
        if bytes[index..].starts_with(b"r#")
            && bytes.get(index + 2).is_some_and(|byte| ident_start(*byte))
        {
            let start = index + 2;
            index = start + 1;
            while index < bytes.len() && ident_continue(bytes[index]) {
                index += 1;
            }
            tokens.push(Token {
                text: source[start..index].to_owned(),
                is_raw_identifier: true,
            });
            continue;
        }
        if ident_start(bytes[index]) {
            let start = index;
            index += 1;
            while index < bytes.len() && ident_continue(bytes[index]) {
                index += 1;
            }
            tokens.push(Token {
                text: source[start..index].to_owned(),
                is_raw_identifier: false,
            });
            continue;
        }
        if bytes[index..].starts_with(b"::") {
            tokens.push(Token {
                text: "::".to_owned(),
                is_raw_identifier: false,
            });
            index += 2;
            continue;
        }
        tokens.push(Token {
            text: (bytes[index] as char).to_string(),
            is_raw_identifier: false,
        });
        index += 1;
    }
    tokens
}

fn production_tokens(source: &str) -> Vec<Token> {
    let tokens = tokenize(source);
    let protected_token_tree = protected_token_tree_mask(&tokens);
    let mut production = Vec::with_capacity(tokens.len());
    let mut cursor = 0;
    while cursor < tokens.len() {
        if !protected_token_tree[cursor]
            && let Some(attribute_end) = cfg_test_attribute_end(&tokens, cursor)
            && let Some(item_end) = cfg_gated_item_end(&tokens, attribute_end)
        {
            cursor = item_end;
        } else {
            production.push(tokens[cursor].clone());
            cursor += 1;
        }
    }
    production
}

fn protected_token_tree_mask(tokens: &[Token]) -> Vec<bool> {
    let mut mask = vec![false; tokens.len()];
    let mut delimiters: Vec<(&str, bool)> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let inside_protected_tree = delimiters.iter().any(|(_, protected)| *protected);
        mask[index] = inside_protected_tree;
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
                let opens_attribute = token.text == "["
                    && tokens
                        .get(index.wrapping_sub(1))
                        .is_some_and(|token| token.text == "#");
                delimiters.push((
                    close,
                    inside_protected_tree || opens_macro || opens_attribute,
                ));
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

fn cfg_gated_item_end(tokens: &[Token], mut cursor: usize) -> Option<usize> {
    // Stop at the first complete construct boundary and never cross an enclosing brace. For a
    // structured match-arm pattern, the first balanced brace can precede the gated body; retaining
    // that body is an intentional fail-closed over-match rather than hiding a production sibling.
    // Angle brackets are likewise not treated as structural depth because `<` is also an operator;
    // a comma in a test-only generic may retain its tail, but cannot hide production code.
    let mut delimiters: Vec<&str> = Vec::new();
    let mut brace_terminates_construct = false;
    let mut saw_item_or_expression_body = false;
    let mut saw_match_arrow = false;
    let mut can_start_item_or_macro = true;
    let mut saw_ambiguous_prefix = false;
    let mut ambiguous_prefix_is_macro_path = true;
    let mut macro_requires_terminator = false;
    while cursor < tokens.len() {
        let macro_body_delimiter = if tokens[cursor].text == "!" {
            let ordinary_macro_delimiter = (cursor > 0
                && saw_ambiguous_prefix
                && ambiguous_prefix_is_macro_path
                && tokens[cursor - 1]
                    .text
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| ident_start(*byte)))
            .then(|| tokens.get(cursor + 1))
            .flatten()
            .filter(|next| matches!(next.text.as_str(), "(" | "[" | "{"));
            ordinary_macro_delimiter
                .or_else(|| {
                    (cursor > 0
                        && tokens[cursor - 1].text == "macro_rules"
                        && !tokens[cursor - 1].is_raw_identifier
                        && tokens.get(cursor + 1).is_some_and(|name| {
                            name.text
                                .as_bytes()
                                .first()
                                .is_some_and(|byte| ident_start(*byte))
                        }))
                    .then(|| tokens.get(cursor + 2))
                    .flatten()
                    .filter(|next| matches!(next.text.as_str(), "(" | "[" | "{"))
                })
                .map(|token| token.text.as_str())
        } else {
            None
        };
        match tokens[cursor].text.as_str() {
            token
                if delimiters.is_empty()
                    && can_start_item_or_macro
                    && !tokens[cursor].is_raw_identifier
                    && matches!(
                        token,
                        "fn" | "mod"
                            | "struct"
                            | "enum"
                            | "union"
                            | "impl"
                            | "trait"
                            | "extern"
                            | "if"
                            | "match"
                            | "loop"
                            | "while"
                            | "for"
                    ) =>
            {
                if saw_ambiguous_prefix {
                    return None;
                }
                saw_item_or_expression_body = true;
            }
            "macro_rules"
                if delimiters.is_empty()
                    && can_start_item_or_macro
                    && !tokens[cursor].is_raw_identifier =>
            {
                if saw_ambiguous_prefix {
                    return None;
                }
            }
            "!" if delimiters.is_empty()
                && can_start_item_or_macro
                && macro_body_delimiter.is_some() =>
            {
                saw_item_or_expression_body = true;
                saw_ambiguous_prefix = false;
                ambiguous_prefix_is_macro_path = true;
                macro_requires_terminator = matches!(macro_body_delimiter, Some("(") | Some("["));
            }
            "!" if delimiters.is_empty() && can_start_item_or_macro => return None,
            "=" if delimiters.is_empty()
                && tokens
                    .get(cursor + 1)
                    .is_some_and(|token| token.text == ">") =>
            {
                saw_match_arrow = true;
            }
            ":" | "=" if delimiters.is_empty() => can_start_item_or_macro = false,
            "(" | "[" | "{" => {
                if delimiters.is_empty() && tokens[cursor].text == "{" {
                    brace_terminates_construct = saw_item_or_expression_body || saw_match_arrow;
                }
                delimiters.push(match tokens[cursor].text.as_str() {
                    "(" => ")",
                    "[" => "]",
                    "{" => "}",
                    _ => unreachable!(),
                });
            }
            ")" | "]" | "}" => {
                let Some(expected) = delimiters.pop() else {
                    return (tokens[cursor].text == "}").then_some(cursor);
                };
                if expected != tokens[cursor].text {
                    return None;
                }
                if delimiters.is_empty()
                    && macro_requires_terminator
                    && matches!(tokens[cursor].text.as_str(), ")" | "]")
                {
                    return match tokens.get(cursor + 1).map(|next| next.text.as_str()) {
                        Some(";" | ",") => Some(cursor + 2),
                        Some("}") | None => Some(cursor + 1),
                        _ => None,
                    };
                }
                if delimiters.is_empty() && tokens[cursor].text == "}" && brace_terminates_construct
                {
                    return Some(cursor + 1);
                }
                if delimiters.is_empty()
                    && tokens[cursor].text == "}"
                    && !brace_terminates_construct
                    && !tokens
                        .get(cursor + 1)
                        .is_some_and(|next| matches!(next.text.as_str(), "," | ";" | "="))
                {
                    return None;
                }
            }
            ";" | "," if delimiters.is_empty() => {
                return Some(cursor + 1);
            }
            token
                if delimiters.is_empty()
                    && can_start_item_or_macro
                    && token
                        .as_bytes()
                        .first()
                        .is_some_and(|byte| ident_start(*byte))
                    && (tokens[cursor].is_raw_identifier
                        || !matches!(
                            token,
                            "pub" | "unsafe" | "async" | "const" | "default" | "auto" | "move"
                        )) =>
            {
                // An identifier may still become a macro path (`foo::bar!`), but it cannot be
                // silently reinterpreted as a modifier for a later item keyword. If no `!`
                // resolves the path before `fn`/`struct`/`mod`/an expression body, retain the
                // entire ambiguous cfg-gated region.
                if saw_ambiguous_prefix
                    && !tokens
                        .get(cursor.wrapping_sub(1))
                        .is_some_and(|previous| previous.text == "::")
                {
                    ambiguous_prefix_is_macro_path = false;
                }
                saw_ambiguous_prefix = true;
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn scan_nested_block_comment(bytes: &[u8], mut index: usize) -> usize {
    let mut depth = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"/*") {
            depth += 1;
            index += 2;
        } else if bytes[index..].starts_with(b"*/") {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return index;
            }
        } else {
            index += 1;
        }
    }
    index
}

fn scan_raw_string(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'b') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;
    let mut hashes = 0;
    while bytes.get(cursor) == Some(&b'#') {
        hashes += 1;
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"'
            && (1..=hashes).all(|offset| bytes.get(cursor + offset) == Some(&b'#'))
        {
            return Some(cursor + hashes + 1);
        }
        cursor += 1;
    }
    Some(cursor)
}

fn scan_quoted_literal(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'b') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' {
            cursor += 2;
        } else if bytes[cursor] == b'"' {
            return Some(cursor + 1);
        } else {
            cursor += 1;
        }
    }
    Some(cursor)
}

fn scan_char_or_lifetime(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'b') && bytes.get(cursor + 1) == Some(&b'\'') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'\'') {
        return None;
    }
    if bytes.get(cursor + 1).is_some_and(|byte| ident_start(*byte))
        && bytes.get(cursor + 2) != Some(&b'\'')
    {
        cursor += 2;
        while cursor < bytes.len() && ident_continue(bytes[cursor]) {
            cursor += 1;
        }
        return Some(cursor);
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' {
            cursor += 2;
        } else if bytes[cursor] == b'\'' {
            return Some(cursor + 1);
        } else {
            cursor += 1;
        }
    }
    Some(cursor)
}

fn texts(tokens: &[Token]) -> Vec<&str> {
    tokens.iter().map(|token| token.text.as_str()).collect()
}

fn count_sequence(tokens: &[Token], expected: &[&str]) -> usize {
    let actual = texts(tokens);
    actual
        .windows(expected.len())
        .filter(|window| *window == expected)
        .count()
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
    violations.extend(
        NT_VENUE_MUTATION_METHOD_NAMES
            .iter()
            .copied()
            .filter(|name| direct_method_reference(&actual, name)),
    );
    violations.extend(
        NT_TRANSITIVE_MUTATION_METHOD_NAMES
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
    // Exact NT command-surface names are reserved by this interim lexical fence. That intentionally
    // rejects same-named local intent enums until the #1407 crate boundary makes NT authority
    // structurally unnameable without relying on source-level resolution.
    tokens.contains(&name)
}

fn bare_function_reference(tokens: &[&str], name: &str) -> bool {
    // Raw transport names are likewise reserved exact tokens. The conservative false-positive
    // direction avoids alias, cast, field, and function-pointer escapes.
    tokens.contains(&name)
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

fn references_retired_registry_type(tokens: &[Token], type_name: &str) -> bool {
    let actual = texts(tokens);
    for start in 0..actual.len().saturating_sub(4) {
        if actual[start..start + 4] == ["strategies", "::", "registry", "::"] {
            if actual.get(start + 4) == Some(&type_name) {
                return true;
            }
            if use_tree_contains_at_depth(&actual, start + 4, type_name) {
                return true;
            }
        }
        if actual[start..start + 3] == ["strategies", "::", "{"] {
            let mut depth = 1usize;
            let mut cursor = start + 3;
            while cursor < actual.len() && depth > 0 {
                match actual[cursor] {
                    "{" => depth += 1,
                    "}" => depth -= 1,
                    "registry"
                        if depth == 1
                            && actual.get(cursor + 1) == Some(&"::")
                            && is_use_tree_segment_start(&actual, cursor) =>
                    {
                        if actual.get(cursor + 2) == Some(&type_name)
                            || use_tree_contains_at_depth(&actual, cursor + 2, type_name)
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
                && name == type_name
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

fn public_declaration_tokens(tokens: &[Token]) -> Vec<&str> {
    let mut public = Vec::new();
    for (start, token) in tokens.iter().enumerate() {
        if token.text != "pub" {
            continue;
        }
        let mut cursor = start;
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut angles = 0usize;
        while let Some(current) = tokens.get(cursor) {
            public.push(current.text.as_str());
            match current.text.as_str() {
                "(" => parentheses += 1,
                ")" => parentheses = parentheses.saturating_sub(1),
                "[" => brackets += 1,
                "]" => brackets = brackets.saturating_sub(1),
                "<" => angles += 1,
                ">" => angles = angles.saturating_sub(1),
                "{" | ";" | "," if parentheses == 0 && brackets == 0 && angles == 0 => break,
                _ => {}
            }
            cursor += 1;
        }
    }
    public
}

#[test]
fn tokenizer_ignores_comments_strings_raw_strings_chars_and_lifetimes() {
    let controls = tokenize(
        r###"
        // crate::strategies::fake::KEY clients.get resolve_fee_provider
        /* nested /* crate::strategies::fake */ clients.get */
        const TEXT: &str = "crate::strategies::fake::KEY clients.get";
        const RAW: &str = r#"resolve_fee_provider crate::strategies"#;
        const CH: char = 'g';
        fn visible<'a>(value: &'a str) { assemble_strategy_build_context(value); }
        "###,
    );
    assert_eq!(count_sequence(&controls, &["crate", "::", "strategies"]), 0);
    assert_eq!(count_sequence(&controls, &["clients", ".", "get"]), 0);
    assert_eq!(count_sequence(&controls, &["resolve_fee_provider"]), 0);
    assert_eq!(
        count_sequence(&controls, &["assemble_strategy_build_context"]),
        1
    );
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
fn production_tokenizer_excludes_inline_test_items_only() {
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
        foo::bar![self.cancel_order(order_id);];
        fn after_cfg_path_macro() { production_after_cfg_path_macro(); }
        #[cfg(test)]
        macro_rules! fixture_rule { () => { self.modify_order(order); } }
        fn after_cfg_macro_rules() { production_after_cfg_macro_rules(); }
        #[cfg(test)]
        if test_mode() { self.modify_order(order); }
        fn after_cfg_expression() { production_after_cfg_expression(); }
        passthrough! { #[cfg(test)] }
        fn after_macro_tokens() { self.modify_order_via_nt(order); }
        #[passthrough(#[cfg(test)])]
        fn after_attribute_tokens() { self.modify_order_via_nt(order); }
        #[passthrough(nested(#[cfg(test)]))]
        fn after_nested_attribute_tokens() { self.modify_order_via_nt(order); }
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
    assert_eq!(count_sequence(&tokens, &["modify_order_via_nt", "("]), 3);
    assert_eq!(count_sequence(&tokens, &["cancel_order", "("]), 1);
    assert_eq!(count_sequence(&tokens, &["test_only_arm_body", "("]), 0);
    assert_eq!(
        count_sequence(&tokens, &["production_after_gated_arm", "("]),
        1
    );
    assert_eq!(
        count_sequence(&tokens, &["production_after_cfg_macro", "("]),
        1
    );
    assert_eq!(
        count_sequence(&tokens, &["production_after_cfg_path_macro", "("]),
        1
    );
    assert_eq!(
        count_sequence(&tokens, &["production_after_cfg_macro_rules", "("]),
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
    for production_sequence in [
        &["struct", "SettlementEvidenceComputation", "{"][..],
        &["fn", "apply_selection_snapshot", "("][..],
        &["self", ".", "config", ".", "warmup_tick_count"][..],
    ] {
        assert!(
            contains_sequence(&tokens, production_sequence),
            "production tokenizer must not hide `{production_sequence:?}` after a test-only field"
        );
    }
}

#[test]
fn strategy_mutation_surface_matcher_has_complete_controls() {
    assert_eq!(
        NT_VENUE_MUTATION_BARE_NAMES,
        &[
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
        ],
        "the independently pinned raw-transport census must not drift"
    );
    let semantic_classes = [
        NT_VENUE_MUTATION_METHOD_NAMES,
        NT_TRANSITIVE_MUTATION_METHOD_NAMES,
        NT_VENUE_MUTATION_BARE_NAMES,
        NT_TRADING_COMMAND_SURFACE_NAMES,
    ];
    for (index, left) in semantic_classes.iter().enumerate() {
        for right in &semantic_classes[index + 1..] {
            assert!(
                !left.iter().any(|name| right.contains(name)),
                "one mutation token must have exactly one matching semantic class"
            );
        }
    }
    for authority in STRATEGY_MUTATION_AUTHORITY_NAMES {
        let tokens = tokenize(&format!("use boundary::{authority};"));
        assert_eq!(named_strategy_mutation_surfaces(&tokens), vec![*authority]);
    }
    for method in NT_VENUE_MUTATION_METHOD_NAMES
        .iter()
        .chain(NT_TRANSITIVE_MUTATION_METHOD_NAMES)
    {
        for source in [
            format!("self.{method}(command);"),
            format!("self.r#{method}(command);"),
            format!("Self::{method}(self, command);"),
            format!("<Wrapper<Foo> as Strategy>::{method}(self, command);"),
            format!("let call = Self::{method} as fn(&mut Self, Command);"),
            format!("let call = &Self::{method};"),
            format!("let calls = [Self::{method}];"),
        ] {
            let tokens = tokenize(&source);
            assert!(
                named_strategy_mutation_surfaces(&tokens).contains(method),
                "direct method matcher must retain {method} in `{source}`"
            );
        }
        let negative = tokenize(&format!(
            "fn {method}() {{}} let {method}_intent = intent; const TEXT: &str = \"{method}\"; // self.{method}(command);"
        ));
        assert!(
            !named_strategy_mutation_surfaces(&negative).contains(method),
            "unqualified definitions, near-neighbors, strings, and comments must not reserve {method}"
        );
    }
    let function_references = tokenize(
        "let exit = Self::exit_market as fn(&mut Self); let markets = [Self::market_exit]; let close = { &self.close_position };",
    );
    for method in ["exit_market", "market_exit", "close_position"] {
        assert!(
            named_strategy_mutation_surfaces(&function_references).contains(&method),
            "function-reference matcher must retain {method}"
        );
    }
    for function in NT_VENUE_MUTATION_BARE_NAMES {
        for source in [
            format!("{function}(command);"),
            format!("r#{function}(command);"),
            format!("msgbus::{function}(command);"),
            format!("msgbus::r#{function}(command);"),
            format!("use msgbus::{function};"),
            format!("use msgbus::{function} as dispatch;"),
            format!("use msgbus::{{{function}}};"),
            format!("use msgbus::{{{function} as dispatch}};"),
            format!("let send = {function} as fn(Command);"),
            format!("let send = msgbus::{function} as fn(Command);"),
            format!("let send = &msgbus::{function};"),
            format!("let sends = [msgbus::{function}];"),
        ] {
            let tokens = tokenize(&source);
            assert!(
                named_strategy_mutation_surfaces(&tokens).contains(function),
                "bare transport matcher must retain {function} in `{source}`"
            );
        }
        let negative = tokenize(&format!(
            "let {function}_intent = intent; let my_{function} = helper; let {function}_v2 = helper; const TEXT: &str = \"{function}\"; // {function}(command);"
        ));
        assert!(
            !named_strategy_mutation_surfaces(&negative).contains(function),
            "near-neighbors, strings, and comments must not reserve {function}"
        );
    }
    for command in NT_TRADING_COMMAND_SURFACE_NAMES {
        for source in [
            command.to_string(),
            format!("r#{command}"),
            format!("nt::{command}"),
            format!("nt::r#{command}"),
            format!("use nt::{command};"),
            format!("use nt::{{{command} as Alias}};"),
            format!("type Pending = Vec<{command}>;"),
            format!("let command = &nt::{command};"),
            format!("let commands = [nt::{command}];"),
        ] {
            let tokens = tokenize(&source);
            assert!(
                named_strategy_mutation_surfaces(&tokens).contains(command),
                "command-surface matcher must retain {command} in `{source}`"
            );
        }
        let negative = tokenize(&format!(
            "let {command}Intent = intent; let my_{command} = helper; const TEXT: &str = \"{command}\"; // nt::{command};"
        ));
        assert!(
            !named_strategy_mutation_surfaces(&negative).contains(command),
            "near-neighbors, strings, and comments must not reserve {command}"
        );
    }
    let command_type_references = tokenize(
        "use nt::{SubmitOrder as NtSubmit}; type Pending = Vec<ModifyOrder>; let constructor = &ClosePosition;",
    );
    for command in ["SubmitOrder", "ModifyOrder", "ClosePosition"] {
        assert!(
            named_strategy_mutation_surfaces(&command_type_references).contains(&command),
            "command aliases, generic positions, and references must retain {command}"
        );
    }
    assert!(named_strategy_mutation_surfaces(&tokenize("emit_order_intent();")).is_empty());
    assert!(
        named_strategy_mutation_surfaces(&tokenize("submit_order_intent();")).is_empty(),
        "near-neighbor intent helper must not trip the exact mutation fence"
    );
    assert_eq!(
        named_strategy_mutation_surfaces(&tokenize(
            "enum ExitAction { CancelOrder } let action = ExitAction::CancelOrder; let choices = [ExitAction::CancelOrder]; match intent { ExitAction::CancelOrder => emit_order_intent() }"
        )),
        vec!["CancelOrder"],
        "exact NT command-surface names stay reserved even for local intent enums"
    );
    assert_eq!(
        named_strategy_mutation_surfaces(&tokenize(
            "enum LocalAction { Emit = generic::<Signal, CancelOrder, Other>() }"
        )),
        vec!["CancelOrder"],
        "commas in generic expressions must not manufacture local-enum exemptions"
    );
    for collision in [
        "enum Cmd { CancelOrder } fn bypass() { use nt::Cmd; let command = Cmd::CancelOrder; }",
        "mod intent { enum Routed { CancelOrder } } fn bypass() { crate::nt_aliases::Routed::CancelOrder(command); }",
    ] {
        assert_eq!(
            named_strategy_mutation_surfaces(&tokenize(collision)),
            vec!["CancelOrder"],
            "unrelated local enums must not exempt NT command references"
        );
    }
}

#[test]
fn production_tokenizer_keeps_ambiguous_test_only_generic_tail_fail_closed() {
    let tokens = production_tokens(
        "struct State { #[cfg(test)] fixture: Map<Id, CancelOrder>, production: ProductionField }",
    );
    assert!(
        contains_sequence(&tokens, &["CancelOrder"]),
        "ambiguous angle brackets stay visible rather than risking production over-skip"
    );
    assert!(contains_sequence(
        &tokens,
        &["production", ":", "ProductionField"]
    ));
}

#[test]
fn production_tokenizer_retains_malformed_cfg_gated_regions() {
    for source in [
        "struct State { #[cfg(test)] fixture: Wrapper<(CancelOrder>, production: SubmitOrder, } fn production_after() {}",
        "struct State { #[cfg(test)] fixture: Wrapper<[CancelOrder>, production: SubmitOrder, } fn production_after() {}",
        "struct State { #[cfg(test)] fixture: Wrapper<{CancelOrder>, production: SubmitOrder, } fn production_after() {}",
        "struct State { #[cfg(test)] fixture: Wrapper<fn(CancelOrder) { production: SubmitOrder, } fn production_after() {}",
    ] {
        let tokens = production_tokens(source);
        assert!(contains_sequence(
            &tokens,
            &["production", ":", "SubmitOrder"]
        ));
        assert!(contains_sequence(&tokens, &["fn", "production_after", "("]));
    }
    for source in [
        "#[cfg(test)] unexpected_prefix fn production() { self.submit_order(order); } fn production_after() {}",
        "#[cfg(test)] unexpected_prefix struct Production { command: SubmitOrder } struct production_after;",
        "#[cfg(test)] unexpected_prefix mod production { fn bypass() { self.cancel_order(id); } } mod production_after {}",
        "#[cfg(test)] unexpected_prefix if enabled { self.modify_order(order); } fn production_after() {}",
        "#[cfg(test)] unexpected_prefix macro_rules! production { () => { self.modify_order(order); } } fn production_after() {}",
    ] {
        let tokens = production_tokens(source);
        assert!(
            contains_sequence(&tokens, &["unexpected_prefix"]),
            "an unknown cfg-gated prefix must retain the ambiguous region"
        );
        assert!(
            !named_strategy_mutation_surfaces(&tokens).is_empty(),
            "an unknown cfg-gated prefix must not hide the fenced mutation token"
        );
        assert!(
            contains_sequence(&tokens, &["production_after"]),
            "an unknown cfg-gated prefix must not hide the production sibling"
        );
    }
    let tokens = production_tokens(
        "#[cfg(test)] foo::bar![self.cancel_order(order_id)] fn production_after() {}",
    );
    assert!(
        !named_strategy_mutation_surfaces(&tokens).is_empty(),
        "a cfg-gated bracket macro without a terminator must retain its mutation token"
    );
    assert!(
        contains_sequence(&tokens, &["production_after"]),
        "a cfg-gated bracket macro without a terminator must not hide its production sibling"
    );
    for modifier in ["pub", "unsafe", "async", "const", "default", "auto", "move"] {
        let source = format!(
            "#[cfg(test)] r#{modifier} fn production() {{ self.submit_order(order); }} fn production_after() {{}}"
        );
        let tokens = production_tokens(&source);
        assert!(
            contains_sequence(&tokens, &[modifier]),
            "a raw modifier-like identifier must retain its lexical token"
        );
        assert!(
            !named_strategy_mutation_surfaces(&tokens).is_empty(),
            "a raw modifier-like identifier must not hide the fenced mutation token"
        );
        assert!(contains_sequence(&tokens, &["production_after"]));
    }
    for source in [
        "#[cfg(test)] unexpected_prefix ! fn production() { self.submit_order(order); } fn production_after() {}",
        "#[cfg(test)] unexpected_prefix ! struct Production { command: SubmitOrder } struct production_after;",
        "#[cfg(test)] unexpected_prefix ! mod production { fn bypass() { self.cancel_order(id); } } mod production_after {}",
    ] {
        let tokens = production_tokens(source);
        assert!(contains_sequence(&tokens, &["unexpected_prefix", "!"]));
        assert!(
            !named_strategy_mutation_surfaces(&tokens).is_empty(),
            "a bang without a macro delimiter must not hide the fenced mutation token"
        );
        assert!(contains_sequence(&tokens, &["production_after"]));
    }
}

#[test]
fn pinned_nt_mutation_surface_is_censused() {
    let lock = std::fs::read_to_string(repo_path("Cargo.lock"))
        .expect("Cargo.lock should retain the audited NT revision");
    assert!(lock.contains("d636f17604cdbddc28ad40e0e15720e2d19bf860"));
    for method in ["modify_orders"] {
        assert!(
            NT_VENUE_MUTATION_METHOD_NAMES.contains(&method),
            "pinned NT Strategy mutation method `{method}` must remain censused"
        );
    }
    for method in [
        "strategy_core_mut",
        "reset_market_exit_state",
        "on_start",
        "on_time_event",
        "check_market_exit",
        "stop",
    ] {
        assert!(
            NT_TRANSITIVE_MUTATION_METHOD_NAMES.contains(&method),
            "pinned NT transitive mutation method `{method}` must remain censused"
        );
    }
    for command in ["ModifyOrders", "BatchModifyOrders", "BatchCancelOrders"] {
        assert!(
            NT_TRADING_COMMAND_SURFACE_NAMES.contains(&command),
            "pinned NT TradingCommand surface `{command}` must remain censused"
        );
    }
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
    for relative in [
        "src/bolt_v3_settlement_booking.rs",
        "src/bolt_v3_runtime_reconcile.rs",
        "src/bolt_v3_reference_price_health.rs",
    ] {
        let tokens = source_tokens(relative);
        let public = public_declaration_tokens(&tokens);
        for name in forbidden {
            assert!(
                !public.contains(&name),
                "{relative} public API exposes forbidden private/handle type `{name}`"
            );
        }
    }
}

#[test]
fn every_archetype_has_one_capability_declaration_and_registration_path() {
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
            count_sequence(&tokens, &["register", ":", "register_runtime_strategy"]),
            1,
            "{relative} must declare one registration callback"
        );
        assert_eq!(
            count_sequence(&tokens, &["fn", "register_runtime_strategy", "("]),
            1,
            "{relative} must define one registration entry point"
        );
        assert_eq!(
            count_sequence(&tokens, &[".", "register_strategy", "("]),
            1,
            "{relative} must have one registry registration path"
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
}
