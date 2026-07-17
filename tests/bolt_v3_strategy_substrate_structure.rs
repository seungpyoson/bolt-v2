//! Secondary structural coverage for the Bolt-v3 strategy-substrate boundary.
//!
//! The Python dependency fence is the primary gate and is the only gate with
//! robust handling for `as` aliases and multi-hop `super` paths. This Rust gate
//! intentionally stays token-sequence based and does not attempt AST parity.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const ARCHETYPES: &[&str] = &[
    "src/strategies/binary_oracle_edge_taker/archetype.rs",
    "src/strategies/binary_oracle_maker/archetype.rs",
    "src/strategies/complete_set_arbitrage/archetype.rs",
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct Token {
    text: String,
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
            });
            continue;
        }
        if bytes[index..].starts_with(b"::") {
            tokens.push(Token {
                text: "::".to_owned(),
            });
            index += 2;
            continue;
        }
        tokens.push(Token {
            text: (bytes[index] as char).to_string(),
        });
        index += 1;
    }
    tokens
}

fn ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_identifier(text: &str) -> bool {
    let Some((first, rest)) = text.as_bytes().split_first() else {
        return false;
    };
    ident_start(*first) && rest.iter().all(|byte| ident_continue(*byte))
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

fn matching_delimiter(
    tokens: &[Token],
    open: usize,
    opening: &str,
    closing: &str,
) -> Option<usize> {
    if tokens.get(open).map(|token| token.text.as_str()) != Some(opening) {
        return None;
    }
    let mut depth = 1usize;
    for (index, token) in tokens.iter().enumerate().skip(open + 1) {
        if token.text == opening {
            depth += 1;
        } else if token.text == closing {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn contains_sequence_at_brace_depth(
    tokens: &[Token],
    expected: &[&str],
    expected_depth: usize,
) -> bool {
    count_sequence_at_brace_depth(tokens, expected, expected_depth) > 0
}

fn count_sequence_at_brace_depth(
    tokens: &[Token],
    expected: &[&str],
    expected_depth: usize,
) -> usize {
    let mut depth = 0usize;
    let mut count = 0usize;
    for start in 0..tokens.len() {
        if tokens[start].text == "}" {
            depth = depth.saturating_sub(1);
        }
        if depth == expected_depth
            && tokens
                .get(start..start + expected.len())
                .is_some_and(|window| texts(window) == expected)
        {
            count += 1;
        }
        if tokens[start].text == "{" {
            depth += 1;
        }
    }
    count
}

fn function_body_tokens<'a>(tokens: &'a [Token], function_name: &str) -> Option<&'a [Token]> {
    let (_, body_start, body_end) = function_definition_span(tokens, function_name)?;
    Some(&tokens[body_start..body_end])
}

fn function_definition_span(
    tokens: &[Token],
    function_name: &str,
) -> Option<(usize, usize, usize)> {
    let function = tokens
        .windows(2)
        .position(|window| window[0].text == "fn" && window[1].text.as_str() == function_name)?;
    let (body_start, body_end) = function_span_at(tokens, function)?;
    Some((function, body_start, body_end))
}

fn function_span_at(tokens: &[Token], function: usize) -> Option<(usize, usize)> {
    let open = tokens[function + 2..]
        .iter()
        .position(|token| token.text == "{")?
        + function
        + 2;
    let mut depth = 1usize;
    for cursor in open + 1..tokens.len() {
        match tokens[cursor].text.as_str() {
            "{" => depth += 1,
            "}" => {
                depth -= 1;
                if depth == 0 {
                    return Some((open + 1, cursor));
                }
            }
            _ => {}
        }
    }
    None
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

fn workspace_crate_files() -> Vec<PathBuf> {
    rust_files_below(&repo_path("crates"))
}

fn is_test_source_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "tests")
}

fn production_strategy_files() -> Vec<PathBuf> {
    rust_files_below(&repo_path("src/strategies"))
        .into_iter()
        .filter(|path| !is_test_source_path(path))
        .collect()
}

fn references_retired_registry_type(tokens: &[Token], type_name: &str) -> bool {
    let actual = texts(tokens);
    let root_aliases = bolt_v2_root_aliases(&actual);
    if root_aliases.len() > 1
        || publicly_reexports_bolt_v2_root(&actual, &root_aliases)
        || imports_bolt_v2_strategies_namespace(&actual, &root_aliases)
    {
        return true;
    }
    for start in 0..actual.len().saturating_sub(2) {
        if actual[start..start + 3] == ["strategies", "::", "registry"] {
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
        if actual[start..start + 3] == ["strategies", "::", "{"] {
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

fn contains_rust_source_indirection(tokens: &[Token]) -> bool {
    contains_rust_include_indirection(tokens) || outer_attribute_contains(tokens, "path")
}

fn rust_include_macro_aliases(tokens: &[Token]) -> BTreeSet<String> {
    let mut aliases = BTreeSet::from(["include".to_string()]);
    loop {
        let discovered = tokens
            .windows(3)
            .filter_map(|window| {
                (window[1].text == "as" && aliases.contains(&window[0].text))
                    .then(|| window[2].text.clone())
            })
            .collect::<Vec<_>>();
        let previous_len = aliases.len();
        aliases.extend(discovered);
        if aliases.len() == previous_len {
            return aliases;
        }
    }
}

fn rust_include_macro_aliases_across_sources(sources: &[Vec<Token>]) -> BTreeSet<String> {
    let mut combined = Vec::new();
    for source in sources {
        combined.extend_from_slice(source);
        combined.push(Token {
            text: ";".to_string(),
        });
    }
    rust_include_macro_aliases(&combined)
}

fn repo_rust_source_tokens() -> Vec<Vec<Token>> {
    rust_files_below(&repo_path("src"))
        .into_iter()
        .chain(workspace_crate_files())
        .map(|path| {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", relative(&path)));
            tokenize(&source)
        })
        .collect()
}

fn contains_rust_include_indirection_with_aliases(
    tokens: &[Token],
    aliases: &BTreeSet<String>,
) -> bool {
    tokens
        .windows(2)
        .any(|window| window[1].text == "!" && aliases.contains(&window[0].text))
}

fn contains_rust_include_indirection(tokens: &[Token]) -> bool {
    let aliases = rust_include_macro_aliases(tokens);
    contains_rust_include_indirection_with_aliases(tokens, &aliases)
}

fn path_attribute_values(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let mut values = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
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
        if !ident_start(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len() && ident_continue(bytes[index]) {
            index += 1;
        }
        if &source[start..index] != "path" {
            continue;
        }
        let mut cursor = index;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            continue;
        }
        let Some(end) = scan_quoted_literal(bytes, cursor) else {
            continue;
        };
        values.push(&source[cursor + 1..end - 1]);
        index = end;
    }
    values
}

fn path_attribute_assignment_count(tokens: &[Token]) -> usize {
    (0..tokens.len())
        .filter_map(|start| outer_attribute_end(tokens, start).map(|end| (start, end)))
        .map(|(start, end)| count_sequence(&tokens[start + 2..end - 1], &["path", "="]))
        .sum()
}

fn path_attributes_resolve_to_scanned_rs(path: &Path, source: &str, tokens: &[Token]) -> bool {
    let values = path_attribute_values(source);
    let assignment_count = path_attribute_assignment_count(tokens);
    let Ok(crates_root) = repo_path("crates").canonicalize() else {
        return false;
    };
    assignment_count > 0
        && values.len() == assignment_count
        && values.into_iter().all(|value| {
            path.parent()
                .map(|parent| parent.join(value))
                .and_then(|target| target.canonicalize().ok())
                .is_some_and(|target| {
                    target.starts_with(&crates_root)
                        && target.extension().and_then(|extension| extension.to_str()) == Some("rs")
                })
        })
}

fn bolt_v2_root_aliases(actual: &[&str]) -> BTreeSet<String> {
    let mut aliases = BTreeSet::from(["bolt_v2".to_string()]);
    loop {
        let previous_len = aliases.len();
        let direct_aliases = actual
            .windows(3)
            .filter_map(|window| {
                (aliases.contains(window[0]) && window[1] == "as").then(|| window[2].to_string())
            })
            .collect::<Vec<_>>();
        aliases.extend(direct_aliases);
        let self_aliases = actual
            .windows(6)
            .filter_map(|window| {
                (aliases.contains(window[0]) && window[1..5] == ["::", "{", "self", "as"])
                    .then(|| window[5].to_string())
            })
            .collect::<Vec<_>>();
        aliases.extend(self_aliases);
        for start in 0..actual.len() {
            let use_start = if actual[start] == "use" {
                Some(start)
            } else if actual[start] == "pub" && actual.get(start + 1) == Some(&"use") {
                Some(start + 1)
            } else {
                None
            };
            if let Some(use_start) = use_start {
                let end = actual[use_start..]
                    .iter()
                    .position(|token| *token == ";")
                    .map_or(actual.len(), |offset| use_start + offset);
                let body = &actual[use_start + 1..end];
                for root in root_use_tree_segments(body, &aliases) {
                    if body.get(root + 1) == Some(&"as") {
                        if let Some(alias) = body.get(root + 2) {
                            aliases.insert((*alias).to_string());
                        }
                    } else if body.get(root + 1..root + 3) == Some(&["::", "{"])
                        && let Some(alias) = use_tree_self_alias(body, root + 2)
                    {
                        aliases.insert(alias.to_string());
                    }
                }
            }
            if actual.get(start..start + 5).is_some_and(|window| {
                window[0..2] == ["extern", "crate"]
                    && window[3] == "as"
                    && aliases.contains(window[2])
            }) {
                aliases.insert(actual[start + 4].to_string());
            }
        }
        if aliases.len() == previous_len {
            return aliases;
        }
    }
}

fn root_use_tree_segments(body: &[&str], aliases: &BTreeSet<String>) -> Vec<usize> {
    if body.first().is_some_and(|root| aliases.contains(*root)) {
        return vec![0];
    }
    if body.first() == Some(&"::") && body.get(1).is_some_and(|root| aliases.contains(*root)) {
        return vec![1];
    }
    if body.first() != Some(&"{") {
        return Vec::new();
    }
    let mut depth = 1usize;
    let mut roots = Vec::new();
    for index in 1..body.len() {
        match body[index] {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            root if depth == 1
                && aliases.contains(root)
                && is_use_tree_segment_start(body, index) =>
            {
                roots.push(index);
            }
            _ => {}
        }
    }
    roots
}

fn use_tree_self_alias<'a>(actual: &[&'a str], open: usize) -> Option<&'a str> {
    if actual.get(open) != Some(&"{") {
        return None;
    }
    let mut depth = 1usize;
    for index in open + 1..actual.len() {
        match actual[index] {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            "self"
                if depth == 1
                    && is_use_tree_segment_start(actual, index)
                    && actual.get(index + 1) == Some(&"as") =>
            {
                return actual.get(index + 2).copied();
            }
            _ => {}
        }
    }
    None
}

fn publicly_reexports_bolt_v2_root(actual: &[&str], aliases: &BTreeSet<String>) -> bool {
    for start in 0..actual.len() {
        if actual.get(start..start + 3) == Some(&["pub", "extern", "crate"])
            && actual
                .get(start + 3)
                .is_some_and(|root| aliases.contains(*root))
        {
            return true;
        }
        if actual.get(start..start + 2) != Some(&["pub", "use"]) {
            continue;
        }
        let end = actual[start + 1..]
            .iter()
            .position(|token| *token == ";")
            .map_or(actual.len(), |offset| start + 1 + offset);
        let body = &actual[start + 2..end];
        for root in root_use_tree_segments(body, aliases) {
            if matches!(
                body.get(root + 1),
                None | Some(&"as") | Some(&",") | Some(&"}")
            ) || body.get(root + 1..root + 3) == Some(&["::", "*"])
                || body.get(root + 1..root + 3) == Some(&["::", "{"])
                    && use_tree_contains_at_depth(body, root + 2, "self")
            {
                return true;
            }
        }
    }
    false
}

fn imports_bolt_v2_strategies_namespace(actual: &[&str], root_aliases: &BTreeSet<String>) -> bool {
    let mut start = 0usize;
    while start < actual.len() {
        if actual[start] != "use"
            && !(actual[start] == "pub" && actual.get(start + 1) == Some(&"use"))
        {
            start += 1;
            continue;
        }
        let end = actual[start..]
            .iter()
            .position(|token| *token == ";")
            .map_or(actual.len(), |offset| start + offset);
        let statement = &actual[start..end];
        for (index, token) in statement.iter().enumerate() {
            if *token != "strategies"
                || !is_bolt_v2_strategies_use_segment(statement, index, root_aliases)
            {
                continue;
            }
            match statement.get(index + 1) {
                None | Some(&"as") | Some(&",") | Some(&"}") => return true,
                Some(&"::") => match statement.get(index + 2) {
                    Some(&"*") => return true,
                    Some(&"{")
                        if use_tree_contains_at_depth(statement, index + 2, "self")
                            || use_tree_contains_at_depth(statement, index + 2, "*") =>
                    {
                        return true;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        start = end.saturating_add(1);
    }
    false
}

fn is_bolt_v2_strategies_use_segment(
    statement: &[&str],
    index: usize,
    root_aliases: &BTreeSet<String>,
) -> bool {
    if index >= 2 && statement[index - 1] == "::" && root_aliases.contains(statement[index - 2]) {
        return true;
    }
    for open in 0..index.saturating_sub(2) {
        if !root_aliases.contains(statement[open])
            || statement.get(open + 1..open + 3) != Some(&["::", "{"])
        {
            continue;
        }
        let mut depth = 1usize;
        for cursor in open + 3..index {
            match statement[cursor] {
                "{" => depth += 1,
                "}" => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if depth == 1 && is_use_tree_segment_start(statement, index) {
            return true;
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

fn public_declarations(tokens: &[Token]) -> Vec<Vec<&str>> {
    let mut declarations = Vec::new();
    for (start, token) in tokens.iter().enumerate() {
        if token.text != "pub" {
            continue;
        }
        let mut declaration = Vec::new();
        let mut cursor = start;
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut angles = 0usize;
        while let Some(current) = tokens.get(cursor) {
            declaration.push(current.text.as_str());
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
        declarations.push(declaration);
    }
    declarations
}

fn normalized_public_declarations(tokens: &[Token]) -> Vec<String> {
    public_declarations(tokens)
        .into_iter()
        .map(|declaration| declaration.join(" "))
        .collect()
}

fn forbidden_aliases(tokens: &[Token], forbidden: &[&str]) -> BTreeMap<String, String> {
    let mut aliases = forbidden
        .iter()
        .map(|name| ((*name).to_string(), (*name).to_string()))
        .collect::<BTreeMap<_, _>>();

    loop {
        let mut discovered = Vec::new();
        for window in tokens.windows(3) {
            if window[1].text == "as"
                && window[2].text != "_"
                && aliases.contains_key(&window[0].text)
            {
                discovered.push((window[2].text.clone(), aliases[&window[0].text].clone()));
            }
        }
        for (index, token) in tokens.iter().enumerate() {
            if token.text != "type" {
                continue;
            }
            let Some(alias) = tokens.get(index + 1) else {
                continue;
            };
            let Some(alias_tail) = tokens.get(index + 2..) else {
                continue;
            };
            let end = alias_tail
                .iter()
                .position(|token| token.text == ";")
                .map_or(tokens.len(), |offset| index + 2 + offset);
            let Some(equals) = tokens[index + 2..end]
                .iter()
                .position(|token| token.text == "=")
                .map(|offset| index + 2 + offset)
            else {
                continue;
            };
            if let Some(root) = tokens[equals + 1..end]
                .iter()
                .find_map(|token| aliases.get(&token.text))
            {
                discovered.push((alias.text.clone(), root.clone()));
            }
        }
        let previous_len = aliases.len();
        aliases.extend(discovered);
        if aliases.len() == previous_len {
            break;
        }
    }
    aliases
}

fn forbidden_aliases_across_sources(
    sources: &[Vec<Token>],
    forbidden: &[&str],
) -> BTreeMap<String, String> {
    let mut combined = Vec::new();
    for source in sources {
        combined.extend_from_slice(source);
        combined.push(Token {
            text: ";".to_string(),
        });
    }
    forbidden_aliases(&combined, forbidden)
}

fn forbidden_public_api_exposures_with_aliases(
    tokens: &[Token],
    aliases: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut exposures = BTreeSet::new();
    for declaration in public_declarations(tokens) {
        if declaration.get(1) == Some(&"type") {
            if let Some(alias) = declaration.get(2)
                && let Some(root) = aliases.get(*alias)
            {
                exposures.insert(((*alias).to_string(), root.clone()));
            }
            continue;
        }
        for name in declaration {
            if let Some(root) = aliases.get(name) {
                exposures.insert((name.to_string(), root.clone()));
            }
        }
    }
    exposures.extend(forbidden_public_body_exposures(tokens, aliases));
    exposures.extend(forbidden_associated_type_exposures(tokens, aliases));
    exposures.into_iter().collect()
}

fn public_item_body_spans(tokens: &[Token], kind: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for start in 0..tokens.len() {
        if tokens[start].text != "pub" {
            continue;
        }
        let mut cursor = start + 1;
        if tokens.get(cursor).map(|token| token.text.as_str()) == Some("(") {
            let Some(close) = matching_delimiter(tokens, cursor, "(", ")") else {
                continue;
            };
            cursor = close + 1;
        }
        while matches!(
            tokens.get(cursor).map(|token| token.text.as_str()),
            Some("unsafe" | "auto")
        ) {
            cursor += 1;
        }
        if tokens.get(cursor).map(|token| token.text.as_str()) != Some(kind) {
            continue;
        }
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut angles = 0usize;
        let open = (cursor + 1..tokens.len()).find(|index| {
            match tokens[*index].text.as_str() {
                "(" => parentheses += 1,
                ")" => parentheses = parentheses.saturating_sub(1),
                "[" => brackets += 1,
                "]" => brackets = brackets.saturating_sub(1),
                "<" => angles += 1,
                ">" => angles = angles.saturating_sub(1),
                "{" if parentheses == 0 && brackets == 0 && angles == 0 => return true,
                ";" if parentheses == 0 && brackets == 0 && angles == 0 => return false,
                _ => {}
            }
            false
        });
        if let Some(open) = open
            && let Some(close) = matching_delimiter(tokens, open, "{", "}")
        {
            spans.push((open, close));
        }
    }
    spans
}

fn forbidden_public_body_exposures(
    tokens: &[Token],
    aliases: &BTreeMap<String, String>,
) -> BTreeSet<(String, String)> {
    let mut exposures = BTreeSet::new();
    for (open, close) in public_item_body_spans(tokens, "trait") {
        let mut cursor = open + 1;
        while cursor < close {
            if tokens[cursor].text == "{" {
                cursor = matching_delimiter(tokens, cursor, "{", "}").map_or(close, |end| end + 1);
                continue;
            }
            if !matches!(tokens[cursor].text.as_str(), "type" | "const" | "fn") {
                cursor += 1;
                continue;
            }
            let Some(item) = tokens.get(cursor + 1) else {
                break;
            };
            let mut parentheses = 0usize;
            let mut brackets = 0usize;
            let mut angles = 0usize;
            let signature_end = (cursor + 2..close)
                .find(|index| {
                    match tokens[*index].text.as_str() {
                        "(" => parentheses += 1,
                        ")" => parentheses = parentheses.saturating_sub(1),
                        "[" => brackets += 1,
                        "]" => brackets = brackets.saturating_sub(1),
                        "<" => angles += 1,
                        ">" => angles = angles.saturating_sub(1),
                        ";" | "{" if parentheses == 0 && brackets == 0 && angles == 0 => {
                            return true;
                        }
                        _ => {}
                    }
                    false
                })
                .unwrap_or(close);
            for value in &tokens[cursor + 2..signature_end] {
                if let Some(root) = aliases.get(&value.text) {
                    exposures.insert((item.text.clone(), root.clone()));
                }
            }
            cursor = signature_end;
        }
    }

    for (open, close) in public_item_body_spans(tokens, "enum") {
        let mut segment_start = open + 1;
        let mut parentheses = 0usize;
        let mut brackets = 0usize;
        let mut braces = 0usize;
        let mut angles = 0usize;
        for segment_end in open + 1..=close {
            let at_end = segment_end == close;
            if !at_end {
                match tokens[segment_end].text.as_str() {
                    "(" => parentheses += 1,
                    ")" => parentheses = parentheses.saturating_sub(1),
                    "[" => brackets += 1,
                    "]" => brackets = brackets.saturating_sub(1),
                    "{" => braces += 1,
                    "}" => braces = braces.saturating_sub(1),
                    "<" => angles += 1,
                    ">" => angles = angles.saturating_sub(1),
                    _ => {}
                }
            }
            let separator = at_end
                || tokens[segment_end].text == ","
                    && parentheses == 0
                    && brackets == 0
                    && braces == 0
                    && angles == 0;
            if !separator {
                continue;
            }
            let mut variant = segment_start;
            while variant < segment_end {
                if let Some(attribute_end) = outer_attribute_end(tokens, variant)
                    && attribute_end <= segment_end
                {
                    variant = attribute_end;
                } else {
                    break;
                }
            }
            if variant < segment_end
                && let Some(name) = tokens.get(variant)
            {
                let payload = tokens[variant + 1..segment_end]
                    .iter()
                    .position(|token| matches!(token.text.as_str(), "(" | "{"))
                    .map(|offset| variant + 1 + offset);
                if let Some(payload) = payload {
                    let (opening, closing) = if tokens[payload].text == "(" {
                        ("(", ")")
                    } else {
                        ("{", "}")
                    };
                    if let Some(payload_end) = matching_delimiter(tokens, payload, opening, closing)
                    {
                        for value in &tokens[payload + 1..payload_end] {
                            if let Some(root) = aliases.get(&value.text) {
                                exposures.insert((name.text.clone(), root.clone()));
                            }
                        }
                    }
                }
            }
            segment_start = segment_end + 1;
        }
    }
    exposures
}

fn forbidden_associated_type_exposures(
    tokens: &[Token],
    aliases: &BTreeMap<String, String>,
) -> BTreeSet<(String, String)> {
    let mut exposures = BTreeSet::new();
    for start in 0..tokens.len() {
        if tokens[start].text != "impl" {
            continue;
        }
        let Some(open) = tokens[start..]
            .iter()
            .position(|token| token.text == "{")
            .map(|offset| start + offset)
        else {
            continue;
        };
        let mut depth = 0usize;
        for index in open + 1..tokens.len() {
            match tokens[index].text.as_str() {
                "{" => depth += 1,
                "}" if depth == 0 => break,
                "}" => depth -= 1,
                "type" if depth == 0 => {
                    let Some(associated) = tokens.get(index + 1) else {
                        continue;
                    };
                    let mut value_depth = 0usize;
                    for value in &tokens[index + 2..] {
                        match value.text.as_str() {
                            "{" => value_depth += 1,
                            "}" if value_depth == 0 => break,
                            "}" => value_depth -= 1,
                            ";" if value_depth == 0 => break,
                            _ => {
                                if let Some(root) = aliases.get(&value.text) {
                                    exposures.insert((associated.text.clone(), root.clone()));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    exposures
}

fn forbidden_public_api_exposures(tokens: &[Token], forbidden: &[&str]) -> Vec<(String, String)> {
    let aliases = forbidden_aliases(tokens, forbidden);
    forbidden_public_api_exposures_with_aliases(tokens, &aliases)
}

fn forbidden_public_reexport_roots(tokens: &[Token]) -> Vec<String> {
    tokens
        .windows(5)
        .filter_map(|window| {
            (texts(&window[..4]) == ["pub", "use", "crate", "::"]
                && !window[4].text.starts_with("bolt_v3_")
                && window[4].text != "source_canonicalization")
                .then(|| window[4].text.clone())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn outer_attribute_end(tokens: &[Token], start: usize) -> Option<usize> {
    if texts(tokens.get(start..start + 2)?) != ["#", "["] {
        return None;
    }
    let mut depth = 1usize;
    let mut cursor = start + 2;
    while cursor < tokens.len() {
        match tokens[cursor].text.as_str() {
            "[" => depth += 1,
            "]" => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn has_preceding_outer_attribute(tokens: &[Token], item_start: usize) -> bool {
    (0..item_start).any(|start| outer_attribute_end(tokens, start) == Some(item_start))
}

fn outer_attribute_contains(tokens: &[Token], name: &str) -> bool {
    (0..tokens.len()).any(|start| {
        outer_attribute_end(tokens, start).is_some_and(|end| {
            tokens[start + 2..end - 1]
                .iter()
                .any(|token| token.text == name)
        })
    })
}

fn function_has_outer_attribute(tokens: &[Token], function: usize) -> bool {
    const ITEM_BOUNDARIES: &[&str] = &[
        ";", "{", "}", "fn", "const", "static", "type", "mod", "impl", "trait", "struct", "enum",
        "union", "use",
    ];
    (0..function).any(|start| {
        outer_attribute_end(tokens, start).is_some_and(|end| {
            end <= function
                && tokens[end..function]
                    .iter()
                    .all(|token| !ITEM_BOUNDARIES.contains(&token.text.as_str()))
        })
    })
}

fn contains_logical_short_circuit(tokens: &[Token]) -> bool {
    for index in 0..tokens.len().saturating_sub(1) {
        if texts(&tokens[index..index + 2]) == ["&", "&"] {
            return true;
        }
        if texts(&tokens[index..index + 2]) != ["|", "|"] {
            continue;
        }
        let closure_prefix = tokens
            .get(index.wrapping_sub(1))
            .map(|token| token.text.as_str())
            .is_some_and(|token| matches!(token, "(" | "=" | "," | "{" | ";" | "move"));
        if !closure_prefix {
            return true;
        }
    }
    false
}

fn top_level_direct_callers(
    tokens: &[Token],
    callee: &str,
) -> (BTreeMap<String, usize>, Vec<String>) {
    let mut callers = BTreeMap::new();
    let mut errors = Vec::new();
    for function in top_level_token_indices(tokens)
        .into_iter()
        .filter(|function| tokens[*function].text == "fn")
    {
        let Some(name) = tokens.get(function + 1).map(|token| token.text.as_str()) else {
            continue;
        };
        if name == callee {
            continue;
        }
        let Some((body_start, body_end)) = function_span_at(tokens, function) else {
            continue;
        };
        let body = &tokens[body_start..body_end];
        let call_count = count_sequence(body, &[callee, "("]);
        if call_count == 0 {
            continue;
        }
        if function_has_outer_attribute(tokens, function)
            || contains_sequence(body, &["cfg"])
            || contains_logical_short_circuit(body)
            || count_sequence_at_brace_depth(body, &[callee, "("], 0) != call_count
        {
            errors.push(format!(
                "{name}: venue_for_client candidate must be a cfg-free direct production call"
            ));
            continue;
        }
        callers.insert(name.to_string(), call_count);
    }
    (callers, errors)
}

fn venue_call_is_followed_by_ok_or_else(tokens: &[Token], call: usize) -> bool {
    let Some(close) = matching_delimiter(tokens, call + 1, "(", ")") else {
        return false;
    };
    tokens
        .get(close + 1..close + 4)
        .is_some_and(|suffix| texts(suffix) == [".", "ok_or_else", "("])
}

fn call_arguments_contain_once(tokens: &[Token], callee: &str, identifier: &str) -> bool {
    let calls = tokens
        .windows(2)
        .enumerate()
        .filter_map(|(index, window)| (texts(window) == [callee, "("]).then_some(index + 1))
        .collect::<Vec<_>>();
    calls.len() == 1
        && matching_delimiter(tokens, calls[0], "(", ")")
            .is_some_and(|end| count_sequence(&tokens[calls[0] + 1..end], &[identifier]) == 1)
}

fn pr1_venue_dataflow_errors(tokens: &[Token]) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(body) = function_body_tokens(tokens, "assemble_strategy_build_context") else {
        return vec!["assemble_strategy_build_context must have a body".to_string()];
    };
    if count_sequence(
        body,
        &["let", "execution_venue", "=", "venue_for_client", "("],
    ) != 1
        || count_sequence(body, &["execution_venue", "="]) != 1
    {
        errors.push(
            "assemble_strategy_build_context must assign venue_for_client once to execution_venue"
                .to_string(),
        );
    }
    let venue_call = body
        .windows(2)
        .position(|window| texts(window) == ["venue_for_client", "("]);
    if !venue_call.is_some_and(|call| venue_call_is_followed_by_ok_or_else(body, call)) {
        errors.push("PR1 venue lookup must fail closed through ok_or_else".to_string());
    }
    let context_call = body
        .windows(4)
        .position(|window| texts(window) == ["StrategyBuildContext", "::", "new", "("]);
    let context_uses_venue = context_call
        .and_then(|call| matching_delimiter(body, call + 3, "(", ")").map(|end| (call + 4, end)))
        .is_some_and(|(start, end)| count_sequence(&body[start..end], &["execution_venue"]) == 1);
    if !context_uses_venue {
        errors.push(
            "StrategyBuildContext::new must consume the resolved execution_venue once".to_string(),
        );
    }
    if count_sequence(body, &["execution_venue"]) != 3
        || !call_arguments_contain_once(
            body,
            "settlement_currency_for_execution_account",
            "execution_venue",
        )
    {
        errors.push(
            "PR1 execution_venue must have exactly its binding, build-context, and settlement-currency roles"
                .to_string(),
        );
    }
    errors
}

fn pr2_venue_dataflow_errors(tokens: &[Token]) -> Vec<String> {
    let Some(body) = function_body_tokens(tokens, "execution_venue_for_context") else {
        return vec!["execution_venue_for_context must have a body".to_string()];
    };
    let Some(call) = body
        .windows(2)
        .position(|window| texts(window) == ["venue_for_client", "("])
    else {
        return vec!["execution_venue_for_context must call venue_for_client".to_string()];
    };
    let direct_expression = (call == 0 || body[call - 1].text == ";")
        && matching_delimiter(body, call + 1, "(", ")")
            .filter(|close| {
                body.get(*close + 1..*close + 4)
                    .is_some_and(|suffix| texts(suffix) == [".", "ok_or_else", "("])
            })
            .and_then(|close| matching_delimiter(body, close + 3, "(", ")"))
            .is_some_and(|ok_or_else_end| ok_or_else_end + 1 == body.len());
    if direct_expression {
        Vec::new()
    } else {
        vec![
            "execution_venue_for_context must return venue_for_client(...).ok_or_else(...) directly"
                .to_string(),
        ]
    }
}

fn pr2_resolver_dataflow_errors(tokens: &[Token]) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(body) = function_body_tokens(tokens, "resolve_settlement_capability") else {
        return vec!["resolve_settlement_capability must have a body".to_string()];
    };
    if count_sequence(
        body,
        &[
            "let",
            "Some",
            "(",
            "execution_venue",
            ")",
            "=",
            "venue_for_client",
            "(",
        ],
    ) != 1
    {
        errors.push(
            "resolve_settlement_capability must bind venue_for_client with let Some(execution_venue)"
                .to_string(),
        );
    }
    let resolved_field = body
        .windows(2)
        .position(|window| texts(window) == ["Resolved", "("])
        .and_then(|resolved| {
            matching_delimiter(body, resolved + 1, "(", ")").map(|end| (resolved + 2, end))
        })
        .is_some_and(|(start, end)| {
            contains_sequence(&body[start..end], &["{", "execution_venue", ","])
        });
    if count_sequence(body, &["execution_venue"]) != 3
        || !call_arguments_contain_once(
            body,
            "settlement_currency_for_execution_account",
            "execution_venue",
        )
        || !resolved_field
    {
        errors.push(
            "PR2 resolver execution_venue must have exactly its binding, settlement-currency, and resolved-resource roles"
                .to_string(),
        );
    }
    errors
}

fn maker_registration_call_surface_errors(tokens: &[Token]) -> Vec<String> {
    let actual = tokens
        .windows(2)
        .filter(|window| window[1].text == "(" && is_identifier(&window[0].text))
        .fold(BTreeMap::new(), |mut calls, window| {
            *calls.entry(window[0].text.as_str()).or_insert(0usize) += 1;
            calls
        });
    let expected = BTreeMap::from([
        ("assemble_strategy_build_context", 1usize),
        ("binding_message", 3),
        ("kind", 1),
        ("kernel", 1),
        ("map_err", 3),
        ("production_strategy_registry", 1),
        ("raw_maker_config", 1),
        ("register_strategy", 1),
        ("to_string", 2),
        ("trader", 1),
    ]);
    if actual == expected {
        Vec::new()
    } else {
        vec![format!(
            "maker register_runtime_strategy call surface differs: expected {expected:?}, found {actual:?}"
        )]
    }
}

fn maker_registration_provenance_errors(tokens: &[Token]) -> Vec<String> {
    let mut errors = Vec::new();
    let canonical_import = [
        "use",
        "crate",
        "::",
        "bolt_v3_strategy_registration",
        "::",
        "{",
        "BoltV3StrategyRegistrationError",
        ",",
        "StrategyRegistrationContext",
        ",",
        "StrategyRuntimeBinding",
        ",",
        "StrategyRuntimeCapabilities",
        ",",
        "assemble_strategy_build_context",
        ",",
        "}",
        ";",
    ];
    if count_sequence(tokens, &canonical_import) != 1
        || count_sequence(tokens, &["bolt_v3_strategy_registration"]) != 1
        || count_sequence(tokens, &["assemble_strategy_build_context"]) != 2
    {
        errors.push(
            "maker must use only the canonical grouped bolt_v3_strategy_registration import"
                .to_string(),
        );
    }
    match function_body_tokens(tokens, "register_runtime_strategy") {
        Some(body) => errors.extend(maker_registration_call_surface_errors(body)),
        None => errors.push("maker register_runtime_strategy must have a body".to_string()),
    }
    errors
}

fn shared_venue_ownership_errors(tokens: &[Token]) -> Vec<String> {
    let mut errors = Vec::new();
    if count_sequence(tokens, &["fn", "venue_for_client", "("]) != 1 {
        errors.push("shared registration must define one venue_for_client".to_string());
        return errors;
    }
    let Some((function, body_start, body_end)) =
        function_definition_span(tokens, "venue_for_client")
    else {
        errors.push("venue_for_client must have a function body".to_string());
        return errors;
    };
    if function_has_outer_attribute(tokens, function) {
        errors.push("venue_for_client must not have an outer attribute".to_string());
    }
    if !top_level_token_indices(tokens).contains(&function) {
        errors.push("venue_for_client must be top-level".to_string());
    }
    if count_sequence(
        &tokens[body_start..body_end],
        &["root", ".", "clients", ".", "get", "("],
    ) != 1
    {
        errors.push("venue_for_client must own one client-table lookup".to_string());
    }
    let account_body = function_definition_span(tokens, "execution_account_id")
        .map(|(_, account_start, account_end)| account_start..account_end);
    if tokens.iter().enumerate().any(|(index, token)| {
        token.text == "clients"
            && !(body_start..body_end).contains(&index)
            && !account_body
                .as_ref()
                .is_some_and(|account| account.contains(&index))
    }) {
        errors.push(
            "clients token appears outside venue_for_client/execution_account_id".to_string(),
        );
    }
    for (index, token) in tokens.iter().enumerate() {
        if token.text != "venue_for_client" || index == function + 1 {
            continue;
        }
        if tokens.get(index + 1).map(|token| token.text.as_str()) != Some("(") {
            errors
                .push("venue_for_client must not be aliased or referenced indirectly".to_string());
        }
        if matches!(
            tokens
                .get(index.wrapping_sub(1))
                .map(|token| token.text.as_str()),
            Some("." | "::")
        ) {
            errors.push("venue_for_client calls must be unqualified and local".to_string());
        }
    }
    let (callers, caller_errors) = top_level_direct_callers(tokens, "venue_for_client");
    errors.extend(caller_errors);
    let pr1_callers = BTreeMap::from([("assemble_strategy_build_context".to_string(), 1usize)]);
    let pr2_callers = BTreeMap::from([
        ("execution_venue_for_context".to_string(), 1usize),
        ("resolve_settlement_capability".to_string(), 1usize),
    ]);
    if callers == pr1_callers {
        errors.extend(pr1_venue_dataflow_errors(tokens));
    } else if callers == pr2_callers {
        errors.extend(pr2_venue_dataflow_errors(tokens));
        errors.extend(pr2_resolver_dataflow_errors(tokens));
    } else {
        errors.push(
            "legitimate direct venue caller set differs; a production function must call venue_for_client"
                .to_string(),
        );
    }
    errors
}

fn top_level_token_indices(tokens: &[Token]) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut brace_depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if token.text == "}" {
            brace_depth = brace_depth.saturating_sub(1);
            continue;
        }
        if brace_depth == 0 {
            indices.push(index);
        }
        if token.text == "{" {
            brace_depth += 1;
        }
    }
    indices
}

fn strategy_bindings_surface_errors(tokens: &[Token]) -> Vec<String> {
    let mut errors = Vec::new();
    let expected = vec![
        "pub fn production_runtime_bindings ( ) - > & [ StrategyRuntimeBinding ] {".to_string(),
        "pub fn production_validation_bindings ( ) - > & [ ArchetypeValidationBinding ] {"
            .to_string(),
    ];
    if normalized_public_declarations(tokens) != expected {
        errors.push("public declarations/signatures differ".to_string());
    }

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
    let top_level = top_level_token_indices(tokens);
    for exact in imports {
        let exact_starts = top_level.iter().copied().filter(|start| {
            tokens
                .get(*start..*start + exact.len())
                .is_some_and(|window| texts(window) == *exact)
        });
        let exact_starts = exact_starts.collect::<Vec<_>>();
        if exact_starts.len() != 1
            || exact_starts
                .first()
                .is_some_and(|start| has_preceding_outer_attribute(tokens, *start))
        {
            errors.push(format!(
                "exact production import differs: {}",
                exact.join("")
            ));
        }
    }

    let top_level_text = top_level
        .iter()
        .map(|index| tokens[*index].text.as_str())
        .collect::<Vec<_>>();
    let const_names = top_level
        .iter()
        .filter_map(|index| {
            (tokens[*index].text == "const")
                .then(|| tokens.get(index + 1).map(|token| token.text.as_str()))
                .flatten()
        })
        .collect::<Vec<_>>();
    let function_names = top_level
        .iter()
        .filter_map(|index| {
            (tokens[*index].text == "fn")
                .then(|| tokens.get(index + 1).map(|token| token.text.as_str()))
                .flatten()
        })
        .collect::<Vec<_>>();
    let module_names = top_level
        .iter()
        .filter_map(|index| {
            (tokens[*index].text == "mod")
                .then(|| tokens.get(index + 1).map(|token| token.text.as_str()))
                .flatten()
        })
        .collect::<Vec<_>>();
    let test_module = top_level.iter().copied().find(|index| {
        tokens[*index].text == "mod"
            && tokens.get(*index + 1).map(|token| token.text.as_str()) == Some("tests")
    });
    let test_module_is_final = test_module.is_some_and(|index| {
        tokens.get(index + 2).map(|token| token.text.as_str()) == Some("{")
            && top_level.last() == Some(&(index + 2))
    });
    let production_end = test_module
        .and_then(|index| index.checked_sub(7))
        .unwrap_or(tokens.len());
    let production_text = texts(&tokens[..production_end]);
    let forbidden_item_heads = [
        "impl",
        "trait",
        "type",
        "struct",
        "enum",
        "union",
        "static",
        "extern",
        "macro",
        "macro_rules",
        "!",
    ];
    let forbidden_production_tokens = [
        "impl",
        "trait",
        "type",
        "struct",
        "enum",
        "union",
        "static",
        "extern",
        "macro",
        "macro_rules",
        "mod",
        "!",
    ];
    if top_level_text
        .iter()
        .filter(|token| **token == "use")
        .count()
        != 3
        || top_level_text
            .iter()
            .filter(|token| **token == "pub")
            .count()
            != 2
        || top_level_text.iter().filter(|token| **token == "#").count() != 1
        || const_names != ["RUNTIME_BINDINGS", "VALIDATION_BINDINGS"]
        || function_names
            != [
                "production_runtime_bindings",
                "production_validation_bindings",
            ]
        || module_names != ["tests"]
        || !test_module_is_final
        || forbidden_item_heads
            .iter()
            .any(|head| top_level_text.contains(head))
        || forbidden_production_tokens
            .iter()
            .any(|token| production_text.contains(token))
        || count_sequence(
            tokens,
            &["#", "[", "cfg", "(", "test", ")", "]", "mod", "tests"],
        ) != 1
    {
        errors.push("unapproved top-level item or attribute in strategy_bindings".to_string());
    }
    if outer_attribute_contains(tokens, "macro_export") {
        errors.push("strategy_bindings must not export macros".to_string());
    }
    errors
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
fn retired_registry_matcher_covers_direct_grouped_wildcard_and_reexport_paths() {
    for source in [
        "use bolt_v2::strategies::registry;",
        "pub use bolt_v2::strategies::registry;",
        "use bolt_v2::strategies::{registry};",
        "pub use bolt_v2::strategies::{registry};",
        "use bolt_v2::strategies::registry as retired_registry;",
        "pub use bolt_v2::strategies::{registry as retired_registry};",
        "use bolt_v2::strategies::registry::{self as retired_registry};",
        "pub use bolt_v2::strategies::{registry::{self as retired_registry}};",
        "use bolt_v2::strategies;",
        "pub use bolt_v2::strategies as shared_strategies;",
        "use bolt_v2::{strategies as shared_strategies};",
        "use bolt_v2::{other, strategies as shared_strategies};",
        "pub use bolt_v2::strategies::*;",
        "use bolt_v2::{strategies::*};",
        "use bolt_v2::{other, strategies::*};",
        "use bolt_v2::strategies::{self as shared_strategies};",
        "use bolt_v2::strategies::registry::FeeProvider;",
        "use bolt_v2::strategies::registry::{FeeProvider, StrategyBuilder};",
        "use bolt_v2::strategies::{registry::FeeProvider, production_strategy_registry};",
        "use bolt_v2::strategies::{registry::{FeeProvider, StrategyBuildContext}};",
        "use bolt_v2::strategies::registry::*;",
        "use bolt_v2::strategies::registry::{*};",
        "use bolt_v2::strategies::{registry::*};",
        "pub use bolt_v2::strategies::registry::FeeProvider;",
        "pub use bolt_v2::strategies::registry::FeeProvider as SharedFeeProvider;",
        "pub use bolt_v2::strategies::registry::*;",
        "use bolt_v2 as b; pub use b::strategies as shared;",
        "use bolt_v2 as b; use b as c; use c::strategies::registry::FeeProvider;",
        "extern crate bolt_v2 as b; use b::strategies::registry::FeeProvider;",
        "use {bolt_v2 as b}; pub use b::strategies as shared;",
        "use bolt_v2::{self as b}; pub use b::strategies as shared;",
        "use bolt_v2 as b; mod child;",
        "use {bolt_v2 as b}; mod child;",
        "use bolt_v2::{self as b}; mod child;",
        "extern crate bolt_v2 as b; mod child;",
        "use ::bolt_v2 as b; mod child;",
        "use {::bolt_v2 as b}; mod child;",
        "pub use bolt_v2 as b;",
        "pub use ::bolt_v2 as b;",
        "pub use {bolt_v2 as b};",
        "pub use bolt_v2::{self as b};",
        "pub extern crate bolt_v2 as b;",
        "use bolt_v2 as b; pub use b as c;",
    ] {
        assert!(references_retired_registry_type(
            &tokenize(source),
            "FeeProvider"
        ));
    }
    for source in [
        "use other::registry::{FeeProvider};",
        "use bolt_v2::strategies::production_strategy_registry;",
        "use bolt_v2::strategies::registry::{nested::FeeProvider};",
        "use bolt_v2::strategies::{nested::{registry::FeeProvider}};",
        "use other::registry::*;",
        "pub use bolt_v2::strategies::registry::nested::*;",
        "pub use bolt_v2::strategies::{nested::registry::*};",
        "use bolt_v2::strategies::registry::nested;",
        "pub use bolt_v2::strategies::{nested::registry};",
        "use other::strategies;",
        "use other::strategies as shared_strategies;",
        "use other::strategies::*;",
        "use {bolt_v2::other, other::strategies as shared_strategies};",
        "use bolt_v2::{other::{strategies as nested_strategies}};",
        "use bolt_v2::strategies::binary_oracle_edge_taker;",
        "use bolt_v2::strategies::{binary_oracle_maker, production_strategy_registry};",
        "use other as b; pub use b::strategies as shared;",
        "extern crate other as b; use b::strategies as shared;",
    ] {
        assert!(!references_retired_registry_type(
            &tokenize(source),
            "FeeProvider"
        ));
    }
}

#[test]
fn workspace_source_indirection_cannot_hide_retired_registry_imports() {
    for source in [
        "include!(\"hidden.inc\");",
        "std::include!(\"hidden.inc\");",
        "#[path = \"hidden.inc\"] mod hidden;",
        "#[cfg_attr(unix, path = \"hidden.inc\")] mod hidden;",
        "use std::include as hidden; hidden!(\"hidden.inc\");",
        "use std::include as hidden; use hidden as nested; nested!(\"hidden.inc\");",
    ] {
        assert!(contains_rust_source_indirection(&tokenize(source)));
    }
    assert!(!contains_rust_source_indirection(&tokenize(
        "#[cfg(unix)] mod ordinary;"
    )));

    let cross_source = vec![
        tokenize("pub use std::include as hidden;"),
        tokenize("use hidden as nested; nested!(\"hidden.inc\");"),
    ];
    let include_aliases = rust_include_macro_aliases_across_sources(&cross_source);
    assert!(contains_rust_include_indirection_with_aliases(
        &cross_source[1],
        &include_aliases,
    ));
    let unscanned_helper = tokenize("pub use std::include as helper_include;");
    let scanned_child = tokenize("helper_include!(\"hidden.inc\");");
    let all_repo_sources = vec![unscanned_helper, scanned_child.clone()];
    let repo_aliases = rust_include_macro_aliases_across_sources(&all_repo_sources);
    assert!(contains_rust_include_indirection_with_aliases(
        &scanned_child,
        &repo_aliases,
    ));

    let mixed = r#"
        #[path = "artifact_store_contract.rs"] mod safe;
        #[path = r"hidden.inc"] mod hidden;
    "#;
    assert!(!path_attributes_resolve_to_scanned_rs(
        &repo_path("crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs"),
        mixed,
        &tokenize(mixed),
    ));

    let spoofed_comment = r#"
        #[path = r"hidden.inc"] mod hidden;
        // path = "artifact_store_contract.rs"
    "#;
    assert!(!path_attributes_resolve_to_scanned_rs(
        &repo_path("crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_tests.rs"),
        spoofed_comment,
        &tokenize(spoofed_comment),
    ));
}

#[test]
fn strategy_bindings_surface_validator_rejects_adversarial_exports() {
    let valid = r#"
        use crate::bolt_v3_archetypes::ArchetypeValidationBinding;
        use crate::bolt_v3_strategy_registration::StrategyRuntimeBinding;
        use crate::strategies::{binary_oracle_edge_taker, binary_oracle_maker, complete_set_arbitrage};
        const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] = &[];
        const VALIDATION_BINDINGS: &[ArchetypeValidationBinding] = &[];
        pub fn production_runtime_bindings() -> &'static [StrategyRuntimeBinding] { loop {} }
        pub fn production_validation_bindings() -> &'static [ArchetypeValidationBinding] { loop {} }
        #[cfg(test)] mod tests {}
    "#;
    assert!(strategy_bindings_surface_errors(&tokenize(valid)).is_empty());

    let adversarial = [
        valid.replace("production_runtime_bindings", "renamed_runtime_bindings"),
        format!("{valid}\npub const EXTRA: usize = 0;"),
        format!("type RuntimeAlias = StrategyRuntimeBinding;\n{valid}").replace(
            "&'static [StrategyRuntimeBinding]",
            "&'static [RuntimeAlias]",
        ),
        valid
            .replace(
                "use crate::bolt_v3_strategy_registration::StrategyRuntimeBinding;",
                "use crate::bolt_v3_strategy_registration::StrategyRuntimeBinding as RuntimeAlias;",
            )
            .replace(
                "&'static [StrategyRuntimeBinding]",
                "&'static [RuntimeAlias]",
            ),
        format!("{valid}\npub use crate::strategies::edge::Strategy;"),
        format!("{valid}\n#[macro_export]\nmacro_rules! exported {{ () => {{}}; }}"),
        valid.replace(
            "use crate::bolt_v3_strategy_registration::StrategyRuntimeBinding;",
            r#"#[cfg(any())]
use crate::bolt_v3_strategy_registration::StrategyRuntimeBinding;
use crate::{bolt_v3_strategy_registration as registration};
use registration::StrategyRuntimeBinding;"#,
        ),
        format!(
            "{valid}\n#[cfg_attr(any(), macro_export)]\nmacro_rules! exported {{ () => {{}}; }}"
        ),
        format!("{valid}\ninclude!(\"hidden_strategy_exports.rs\");"),
        format!("{valid}\nmod hidden_strategy_exports {{}}"),
        format!(
            "{valid}\nimpl IntoIterator for StrategyRuntimeBinding {{\n\
             type Item = crate::strategies::binary_oracle_maker::BinaryOracleMakerBuilder;\n\
             type IntoIter = std::iter::Empty<Self::Item>;\n\
             fn into_iter(self) -> Self::IntoIter {{ std::iter::empty() }}\n\
             }}"
        ),
        valid.replace(
            "const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] = &[];",
            "const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] = { include!(\"hidden.rs\"); &[] };",
        ),
        valid.replace(
            "pub fn production_runtime_bindings() -> &'static [StrategyRuntimeBinding] { loop {} }",
            "pub fn production_runtime_bindings() -> &'static [StrategyRuntimeBinding] { impl IntoIterator for StrategyRuntimeBinding { type Item = crate::strategies::binary_oracle_maker::BinaryOracleMakerBuilder; type IntoIter = std::iter::Empty<Self::Item>; fn into_iter(self) -> Self::IntoIter { std::iter::empty() } } loop {} }",
        ),
        valid.replace("#[cfg(test)] mod tests {}", "").replace(
            "pub fn production_runtime_bindings() -> &'static [StrategyRuntimeBinding] { loop {} }",
            "#[cfg(test)] mod tests {} pub fn production_runtime_bindings() -> &'static [StrategyRuntimeBinding] { include!(\"hidden.rs\"); loop {} }",
        ),
    ];
    for source in adversarial {
        assert!(!strategy_bindings_surface_errors(&tokenize(&source)).is_empty());
    }
}

#[test]
fn public_api_forbidden_type_aliases_cannot_launder_handle_types() {
    let forbidden = ["LiveNodeHandle", "DataActor"];
    for source in [
        "use nautilus_live::node::LiveNodeHandle as NeutralHandle; pub fn leak() -> NeutralHandle { loop {} }",
        "type NeutralHandle = LiveNodeHandle; pub fn leak() -> NeutralHandle { loop {} }",
        "use nautilus_live::node::LiveNodeHandle as HiddenHandle; type NeutralHandle = HiddenHandle; pub fn leak() -> NeutralHandle { loop {} }",
        "type HiddenHandle = LiveNodeHandle; pub type NeutralHandle = HiddenHandle;",
        "type NeutralHandle<T> = LiveNodeHandle; pub fn leak() -> NeutralHandle<()> { loop {} }",
        "type HiddenHandle<'a, T> = LiveNodeHandle; type NeutralHandle<U> = HiddenHandle<'static, U>; pub fn leak() -> NeutralHandle<()> { loop {} }",
    ] {
        let exposures = forbidden_public_api_exposures(&tokenize(source), &forbidden);
        assert_eq!(
            exposures,
            vec![("NeutralHandle".to_string(), "LiveNodeHandle".to_string())]
        );
    }

    let unrelated = tokenize(
        "use safe::Handle as NeutralHandle; type OtherHandle = Handle; pub fn safe() -> NeutralHandle { loop {} }",
    );
    assert!(forbidden_public_api_exposures(&unrelated, &forbidden).is_empty());

    let sources = vec![
        tokenize("pub type NeutralHandle = LiveNodeHandle;"),
        tokenize("pub fn leak() -> NeutralHandle { loop {} }"),
    ];
    let aliases = forbidden_aliases_across_sources(&sources, &forbidden);
    assert_eq!(
        forbidden_public_api_exposures_with_aliases(&sources[1], &aliases),
        vec![("NeutralHandle".to_string(), "LiveNodeHandle".to_string())]
    );

    let associated = tokenize(
        "pub trait Leak { type Output; } pub struct Public; \
         impl Leak for Public { type Output = LiveNodeHandle; }",
    );
    assert_eq!(
        forbidden_public_api_exposures(&associated, &forbidden),
        vec![("Output".to_string(), "LiveNodeHandle".to_string())]
    );

    let associated_alias = tokenize(
        "type Hidden = LiveNodeHandle; pub trait Leak { type Output; } pub struct Public; \
         impl Leak for Public { type Output = Hidden; }",
    );
    assert_eq!(
        forbidden_public_api_exposures(&associated_alias, &forbidden),
        vec![("Output".to_string(), "LiveNodeHandle".to_string())]
    );

    for (source, item) in [
        (
            "pub trait Leak { fn handle(&self) -> LiveNodeHandle; }",
            "handle",
        ),
        ("pub enum Leak { Handle(LiveNodeHandle) }", "Handle"),
        (
            "pub unsafe trait Leak { type Output: Trait<LiveNodeHandle>; }",
            "Output",
        ),
    ] {
        assert_eq!(
            forbidden_public_api_exposures(&tokenize(source), &forbidden),
            vec![(item.to_string(), "LiveNodeHandle".to_string())]
        );
    }

    assert_eq!(
        forbidden_public_reexport_roots(&tokenize("pub use crate::helper::Leak;")),
        vec!["helper".to_string()]
    );
    for allowed in [
        "pub use crate::bolt_v3_other::Leak;",
        "pub use crate::source_canonicalization::Leak;",
    ] {
        assert!(forbidden_public_reexport_roots(&tokenize(allowed)).is_empty());
    }
}

#[test]
fn strategy_bindings_exports_only_the_exact_production_binding_signatures() {
    let tokens = source_tokens("src/strategy_bindings.rs");
    let errors = strategy_bindings_surface_errors(&tokens);
    assert!(
        errors.is_empty(),
        "strategy_bindings public surface drifted: {errors:?}"
    );
}

#[test]
fn function_body_extraction_is_symbol_scoped() {
    let tokens = tokenize(
        "fn target() { root.clients.get(); } fn control() { root.clients.get(); root.clients.get(); }",
    );
    let body = function_body_tokens(&tokens, "target").expect("target function should exist");
    assert_eq!(
        count_sequence(body, &["root", ".", "clients", ".", "get", "("]),
        1
    );
}

#[test]
fn venue_ownership_rejects_a_test_only_accessor_decoy() {
    let tokens = tokenize(
        "fn alternate_venue(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         fn production() { alternate_venue(); } \
         #[cfg(test)] fn venue_for_client(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         #[cfg(test)] fn control() { venue_for_client(); }",
    );
    let errors = shared_venue_ownership_errors(&tokens);
    assert!(
        errors.iter().any(|error| error.contains("outer attribute")),
        "{errors:?}"
    );

    let unreferenced = tokenize(
        "fn venue_for_client(root: &Root, client_id: &str) { root.clients.get(client_id); }",
    );
    let errors = shared_venue_ownership_errors(&unreferenced);
    assert!(
        errors.iter().any(|error| error.contains("must call")),
        "{errors:?}"
    );

    let nested = tokenize(
        "#[cfg(test)] mod tests { \
         fn venue_for_client(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         fn control() { venue_for_client(); } \
         }",
    );
    let errors = shared_venue_ownership_errors(&nested);
    assert!(
        errors.iter().any(|error| error.contains("top-level")),
        "{errors:?}"
    );

    let test_only_caller = tokenize(
        "fn venue_for_client(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         #[cfg(test)] mod tests { fn control() { super::venue_for_client(); } }",
    );
    let errors = shared_venue_ownership_errors(&test_only_caller);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("production function")),
        "{errors:?}"
    );

    for caller in [
        "fn production() { if cfg!(test) { venue_for_client(); } }",
        "fn production() { if enabled { venue_for_client(); } }",
        "fn production() { false && venue_for_client(); }",
        "fn production() { true || venue_for_client(); }",
        "fn production() { let wrapper = || { venue_for_client(); }; wrapper(); }",
    ] {
        let source = format!(
            "fn venue_for_client(root: &Root, client_id: &str) {{ root.clients.get(client_id); }} {caller}"
        );
        let errors = shared_venue_ownership_errors(&tokenize(&source));
        assert!(
            errors
                .iter()
                .any(|error| error.contains("direct production")),
            "{errors:?}"
        );
    }

    let wrapped_caller = tokenize(
        "fn venue_for_client(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         fn wrapper() { venue_for_client(); } \
         fn assemble_strategy_build_context() { wrapper(); }",
    );
    let errors = shared_venue_ownership_errors(&wrapped_caller);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("caller set differs")),
        "{errors:?}"
    );

    let pr2_callers = tokenize(
        "fn venue_for_client(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         fn resolve_settlement_capability(loaded: &Loaded, execution_client_id: &str) { \
             let Some(execution_venue) = venue_for_client(&loaded.root, execution_client_id) else { \
                 return Invalid; \
             }; \
             let Some(settlement_currency) = settlement_currency_for_execution_account( \
                 &loaded.root, execution_venue, account_id, \
             ) else { return Invalid; }; \
             Resolved(Resources { execution_venue, settlement_currency }); \
         } \
         fn execution_venue_for_context(context: &Context) -> Result<Venue, Error> { \
             let execution_client_id = context.execution_client_id; \
             venue_for_client(&context.root, execution_client_id).ok_or_else(|| error()) \
         } \
         fn settlement_resources_for_context() { settlement_only(); }",
    );
    assert!(
        shared_venue_ownership_errors(&pr2_callers).is_empty(),
        "the PR2 venue caller surface must remain accepted"
    );

    for caller in [
        "fn assemble_strategy_build_context() { \
             let execution_venue = self::venue_for_client(root, client_id).ok_or_else(|| error())?; \
             StrategyBuildContext::new(a, b, c, d, execution_venue); \
         }",
        "fn assemble_strategy_build_context() { \
             let execution_venue = venue_for_client(root, client_id).ok_or_else(|| error())?; \
             venue_for_client(root, client_id); \
             StrategyBuildContext::new(a, b, c, d, execution_venue); \
         }",
        "fn assemble_strategy_build_context() { \
             venue_for_client(root, client_id); \
             let execution_venue = alternate_venue(); \
             StrategyBuildContext::new(a, b, c, d, execution_venue); \
         }",
        "fn assemble_strategy_build_context() { \
             let alternate = venue_for_client(root, client_id).ok_or_else(|| error())?; \
             StrategyBuildContext::new(a, b, c, d, alternate); \
         }",
        "fn assemble_strategy_build_context() { \
             let execution_venue = venue_for_client(root, client_id).ok_or_else(|| error())?; \
             StrategyBuildContext::new(a, b, c, d, execution_venue); \
             settlement_currency_for_execution_account(root, execution_venue, account_id); \
             let [execution_venue] = [alternate_venue()]; \
         }",
    ] {
        let source = format!(
            "fn venue_for_client(root: &Root, client_id: &str) {{ root.clients.get(client_id); }} {caller}"
        );
        let errors = shared_venue_ownership_errors(&tokenize(&source));
        assert!(
            !errors.is_empty(),
            "venue dataflow bypass accepted: {caller}"
        );
    }

    let alternate_lookup = tokenize(
        "fn venue_for_client(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         fn execution_account_id(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         fn alternate(root: &Root, client_id: &str) { root.clients.get(client_id); } \
         fn production() { venue_for_client(); }",
    );
    let errors = shared_venue_ownership_errors(&alternate_lookup);
    assert!(
        errors.iter().any(|error| error.contains("outside")),
        "{errors:?}"
    );

    let maker = source_tokens("src/strategies/binary_oracle_maker/archetype.rs");
    let maker_body = function_body_tokens(&maker, "register_runtime_strategy")
        .expect("maker registration function should exist");
    assert!(maker_registration_call_surface_errors(maker_body).is_empty());
    assert!(maker_registration_provenance_errors(&maker).is_empty());
    let mut wrapped = maker_body.to_vec();
    wrapped.extend(tokenize("renamed_wrapper();"));
    assert!(!maker_registration_call_surface_errors(&wrapped).is_empty());
    let mut aliased_import = maker.clone();
    aliased_import.extend(tokenize(
        "use alternate::assemble_strategy_build_context as shared_assemble;",
    ));
    assert!(!maker_registration_provenance_errors(&aliased_import).is_empty());
}

#[test]
fn strategy_client_map_matcher_covers_non_get_access_paths() {
    for source in [
        "root.clients.iter();",
        "root.clients.values();",
        "root.clients[client_id];",
        "helper(&root.clients);",
    ] {
        assert_eq!(count_sequence(&tokenize(source), &["clients"]), 1);
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
    }
}

#[test]
fn production_strategy_layer_never_accesses_the_client_map_directly() {
    let mut violations = Vec::new();
    for path in production_strategy_files() {
        let source = std::fs::read_to_string(&path).expect("strategy source should be readable");
        if contains_sequence(&tokenize(&source), &["clients"]) {
            violations.push(relative(&path));
        }
    }
    assert!(
        violations.is_empty(),
        "production strategy files access the client map directly: {violations:?}"
    );
}

#[test]
fn production_bolt_and_strategy_sources_use_no_rust_source_indirection() {
    let mut violations = Vec::new();
    let sources = production_bolt_v3_files()
        .into_iter()
        .chain(production_strategy_files())
        .filter(|path| !is_test_source_path(path))
        .map(|path| {
            let source =
                std::fs::read_to_string(&path).expect("production source should be readable");
            let tokens = tokenize(&source);
            (path, tokens)
        })
        .collect::<Vec<_>>();
    let include_aliases = rust_include_macro_aliases_across_sources(&repo_rust_source_tokens());
    for (path, tokens) in sources {
        if contains_rust_include_indirection_with_aliases(&tokens, &include_aliases)
            || outer_attribute_contains(&tokens, "path")
        {
            violations.push(relative(&path));
        }
    }
    assert!(
        violations.is_empty(),
        "production Bolt-v3/strategy source indirection found: {violations:?}"
    );
}

#[test]
fn shared_strategy_registration_owns_execution_venue_resolution() {
    let tokens = source_tokens("src/bolt_v3_strategy_registration.rs");
    let errors = shared_venue_ownership_errors(&tokens);
    assert!(
        errors.is_empty(),
        "shared strategy registration venue ownership drifted: {errors:?}"
    );
    let maker = source_tokens("src/strategies/binary_oracle_maker/archetype.rs");
    assert_eq!(
        count_sequence(&maker, &["venue_for_client"]),
        0,
        "the maker must not duplicate shared execution-venue resolution"
    );
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
    let sources = workspace_crate_files()
        .into_iter()
        .map(|path| {
            let source =
                std::fs::read_to_string(&path).expect("workspace source should be readable");
            let tokens = tokenize(&source);
            (path, source, tokens)
        })
        .collect::<Vec<_>>();
    let include_aliases = rust_include_macro_aliases_across_sources(&repo_rust_source_tokens());
    for (path, source, tokens) in sources {
        let source_indirection =
            contains_rust_include_indirection_with_aliases(&tokens, &include_aliases)
                || outer_attribute_contains(&tokens, "path")
                    && !path_attributes_resolve_to_scanned_rs(&path, &source, &tokens);
        if source_indirection {
            violations.push(format!(
                "{}: Rust source indirection via include!/path attribute",
                relative(&path)
            ));
        }
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
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} should be readable: {error}", relative(&path)));
            tokenize(&source)
        })
        .collect::<Vec<_>>();
    let aliases = forbidden_aliases_across_sources(&crate_sources, &forbidden);
    let mut scanned = Vec::new();
    let mut public_api_files = production_bolt_v3_files();
    public_api_files.push(repo_path("src/source_canonicalization.rs"));
    for path in public_api_files {
        let relative = relative(&path);
        let tokens = source_tokens(&relative);
        if relative.starts_with("src/bolt_v3_") {
            let forbidden_reexports = forbidden_public_reexport_roots(&tokens);
            assert!(
                forbidden_reexports.is_empty(),
                "{relative} publicly reexports unscanned crate roots: {forbidden_reexports:?}"
            );
        }
        if relative == "src/bolt_v3_live_node.rs" || relative.starts_with("src/bolt_v3_live_node/")
        {
            continue;
        }
        let exposures = forbidden_public_api_exposures_with_aliases(&tokens, &aliases);
        assert!(
            exposures.is_empty(),
            "{relative} public API exposes forbidden private/handle type aliases: {exposures:?}"
        );
        scanned.push(relative);
    }
    assert!(
        scanned.contains(&"src/bolt_v3_submit_admission.rs".to_string()),
        "shared public-API scan must cover submit admission"
    );
    assert!(
        scanned.contains(&"src/source_canonicalization.rs".to_string()),
        "shared public-API scan must cover the allowed non-Bolt reexport source"
    );
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
