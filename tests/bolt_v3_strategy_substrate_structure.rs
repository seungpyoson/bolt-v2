use std::path::{Path, PathBuf};

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
    if bytes
        .get(cursor + 1)
        .is_some_and(|byte| ident_start(*byte))
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
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
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
            name
                if depth == 1
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
    assert!(!references_retired_registry_type(
        &unrelated,
        "FeeProvider"
    ));
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
        &["crate", "::", "bolt_v3_archetypes", "::", "binary_oracle_edge_taker"],
        &["crate", "::", "bolt_v3_archetypes", "::", "binary_oracle_maker"],
        &["crate", "::", "bolt_v3_maker_settlement"],
        &["crate", "::", "bolt_v3_maker_runtime_settlement"],
        &["crate", "::", "strategies", "::", "binary_oracle_edge_taker", "::", "settlement"],
        &["crate", "::", "strategies", "::", "binary_oracle_edge_taker", "::", "settlement_booking"],
        &["crate", "::", "strategies", "::", "complete_set_arbitrage", "::", "settlement"],
        &["crate", "::", "strategies", "::", "complete_set_arbitrage", "::", "settlement_booking"],
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
    assert!(violations.is_empty(), "obsolete full paths remain: {violations:?}");
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
    let dependency_fence = std::fs::read_to_string(repo_path(
        "scripts/verify_bolt_v3_dependency_direction.py",
    ))
    .expect("dependency fence should be readable");
    assert!(dependency_fence.lines().any(|line| {
        line.trim() == "FINDING_ALLOWANCES: tuple[FindingAllowance, ...] = ()"
    }));
    let runner = std::fs::read_to_string(repo_path("scripts/run_fences.py"))
        .expect("source-fence runner should be readable");
    assert!(runner.contains("scripts_dir.glob(\"verify_*.py\")"));
    assert!(dependency_fence.contains("enforce_shrink_only = argv is None"));
    assert!(dependency_fence.contains("return check_allowlist_shrink_only()"));
}
