use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use quote::ToTokens;
use syn::{
    Attribute, ForeignItem, ImplItem, Item, Meta, Token, TraitItem,
    parse::Parser,
    punctuated::Punctuated,
    visit::{self, Visit},
    visit_mut::{self, VisitMut},
};

const SAMPLE_VENUE_NEEDLES: [&str; 15] = [
    "bybit",
    "binance",
    "bnbusdc",
    "pmxt",
    "polymarket",
    "public_archive",
    "upbit",
    "bithumb",
    "korbit",
    "coinone",
    "kimchi",
    "korean_spot",
    "reference_price",
    "fx_quote",
    "token_mapping",
];

#[test]
fn production_rust_does_not_hardcode_sample_venue_or_instrument() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = rust_sources(&src);
    let failures = sample_venue_violations(&src, &sources);

    assert!(
        failures.is_empty(),
        "sample venue/instrument values must stay in TOML reference fixtures, tests, or explicit one-off proof modules, not generic production Rust:\n{}",
        failures.join("\n")
    );
}

fn sample_venue_violations(src: &Path, sources: &[(PathBuf, String)]) -> Vec<String> {
    let test_only_includes = test_only_included_source_paths(src, sources);
    let mut failures = Vec::new();
    for (path, content) in sources {
        if test_only_includes.contains(path) {
            continue;
        }
        let lower = production_source(content).to_ascii_lowercase();
        for needle in SAMPLE_VENUE_NEEDLES {
            if lower.contains(needle) && !needle_allowed_in_production_path(needle, path, src) {
                failures.push(format!("{} contains {needle:?}", path.display()));
            }
        }
    }
    failures
}

#[test]
fn run_manifest_unit_tests_do_not_embed_accepted_sample_fixture_values() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/run_manifest.rs");
    let content = fs::read_to_string(&path).expect("read run manifest source");
    let file = syn::parse_file(&content).expect("parse run manifest source");
    let unit_tests = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Mod(module)
                if module.ident == "tests" && attributes_require_test(&module.attrs) =>
            {
                Some(module.to_token_stream().to_string())
            }
            _ => None,
        })
        .expect("cfg(test)-gated run_manifest unit tests");
    let lower = unit_tests.to_ascii_lowercase();
    let mut failures = Vec::new();
    for needle in ["bybit", "bnbusdc", "public_archive"] {
        if lower.contains(needle) {
            failures.push(needle);
        }
    }

    assert!(
        failures.is_empty(),
        "generic run_manifest unit fixtures must use synthetic values, not the accepted sample proof values: {}",
        failures.join(", ")
    );
}

#[test]
fn committed_pack_completion_boundaries_are_registry_derived_not_venue_listed() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (relative_path, function_name) in [
        (
            "src/source_universe_batch_launch.rs",
            "discover_committed_source_universe_execution_packs",
        ),
        (
            "src/bin/source_universe_batch_execution.rs",
            "committed_one_record_launch_profiles_select_exact_staged_s3_packs",
        ),
        (
            "src/source_universe_object_transport.rs",
            "committed_tracers_plan_only_their_staged_s3_object",
        ),
        (
            "tests/backtesting_vertical_slice_source_universe_execution_acceptance.rs",
            "committed_execution_pack_registry_and_acceptance_ledger_are_an_exact_set",
        ),
    ] {
        let path = crate_root.join(relative_path);
        let source = fs::read_to_string(&path).expect("read registry-derived boundary source");
        let function = rust_function_region(&source, function_name);
        let lower = function.to_ascii_lowercase();
        for venue in ["binance", "bybit"] {
            assert!(
                !lower.contains(venue),
                "generic committed-pack boundary {function_name} in {} must discover registry entries, not list venue {venue}",
                path.display()
            );
        }
    }
}

fn rust_function_region<'a>(source: &'a str, function_name: &str) -> &'a str {
    let signature = format!("fn {function_name}(");
    let function_start = source
        .find(&signature)
        .unwrap_or_else(|| panic!("missing function {function_name}"));
    let source = &source[function_start..];
    let body_start = source
        .find('{')
        .unwrap_or_else(|| panic!("missing body for function {function_name}"));
    let mut depth = 0_u64;
    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1).expect("balanced function braces");
                if depth == 0 {
                    return &source[..body_start + offset + character.len_utf8()];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated body for function {function_name}")
}

fn production_source(content: &str) -> String {
    let mut file = syn::parse_file(content).expect("parse Rust source for cfg reachability");
    let mut pruner = ProductionItemPruner;
    pruner.visit_file_mut(&mut file);
    file.into_token_stream().to_string()
}

struct ProductionItemPruner;

impl VisitMut for ProductionItemPruner {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        file.items
            .retain(|item| !attributes_require_test(item_attributes(item)));
        for item in &mut file.items {
            self.visit_item_mut(item);
        }
    }

    fn visit_item_mod_mut(&mut self, item: &mut syn::ItemMod) {
        if let Some((_, items)) = &mut item.content {
            items.retain(|item| !attributes_require_test(item_attributes(item)));
            for item in items {
                self.visit_item_mut(item);
            }
        }
    }

    fn visit_item_impl_mut(&mut self, item: &mut syn::ItemImpl) {
        item.items
            .retain(|item| !attributes_require_test(impl_item_attributes(item)));
        for item in &mut item.items {
            visit_mut::visit_impl_item_mut(self, item);
        }
    }

    fn visit_item_trait_mut(&mut self, item: &mut syn::ItemTrait) {
        item.items
            .retain(|item| !attributes_require_test(trait_item_attributes(item)));
        for item in &mut item.items {
            visit_mut::visit_trait_item_mut(self, item);
        }
    }

    fn visit_item_foreign_mod_mut(&mut self, item: &mut syn::ItemForeignMod) {
        item.items
            .retain(|item| !attributes_require_test(foreign_item_attributes(item)));
        for item in &mut item.items {
            visit_mut::visit_foreign_item_mut(self, item);
        }
    }
}

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Verbatim(_) => &[],
        _ => &[],
    }
}

fn impl_item_attributes(item: &ImplItem) -> &[Attribute] {
    match item {
        ImplItem::Const(item) => &item.attrs,
        ImplItem::Fn(item) => &item.attrs,
        ImplItem::Macro(item) => &item.attrs,
        ImplItem::Type(item) => &item.attrs,
        ImplItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn trait_item_attributes(item: &TraitItem) -> &[Attribute] {
    match item {
        TraitItem::Const(item) => &item.attrs,
        TraitItem::Fn(item) => &item.attrs,
        TraitItem::Macro(item) => &item.attrs,
        TraitItem::Type(item) => &item.attrs,
        TraitItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn foreign_item_attributes(item: &ForeignItem) -> &[Attribute] {
    match item {
        ForeignItem::Fn(item) => &item.attrs,
        ForeignItem::Macro(item) => &item.attrs,
        ForeignItem::Static(item) => &item.attrs,
        ForeignItem::Type(item) => &item.attrs,
        ForeignItem::Verbatim(_) => &[],
        _ => &[],
    }
}

fn attributes_require_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(cfg_attribute_requires_test)
}

fn cfg_attribute_requires_test(attribute: &Attribute) -> bool {
    if !attribute.path().is_ident("cfg") {
        return false;
    }
    attribute
        .parse_args::<Meta>()
        .ok()
        .is_some_and(|meta| !cfg_possibility_when_test_is_disabled(&meta).can_be_true)
}

#[derive(Clone, Copy)]
struct CfgPossibility {
    can_be_true: bool,
    can_be_false: bool,
}

const UNKNOWN_CFG: CfgPossibility = CfgPossibility {
    can_be_true: true,
    can_be_false: true,
};

fn cfg_possibility_when_test_is_disabled(meta: &Meta) -> CfgPossibility {
    match meta {
        Meta::Path(path) if path.is_ident("test") => CfgPossibility {
            can_be_true: false,
            can_be_false: true,
        },
        Meta::Path(_) | Meta::NameValue(_) => UNKNOWN_CFG,
        Meta::List(list) => {
            let operation = if list.path.is_ident("all") {
                "all"
            } else if list.path.is_ident("any") {
                "any"
            } else if list.path.is_ident("not") {
                "not"
            } else {
                return UNKNOWN_CFG;
            };
            let Ok(arguments) =
                Punctuated::<Meta, Token![,]>::parse_terminated.parse2(list.tokens.clone())
            else {
                return UNKNOWN_CFG;
            };
            match operation {
                "all" => CfgPossibility {
                    can_be_true: arguments.iter().all(|argument| {
                        cfg_possibility_when_test_is_disabled(argument).can_be_true
                    }),
                    can_be_false: arguments.iter().any(|argument| {
                        cfg_possibility_when_test_is_disabled(argument).can_be_false
                    }),
                },
                "any" => CfgPossibility {
                    can_be_true: arguments.iter().any(|argument| {
                        cfg_possibility_when_test_is_disabled(argument).can_be_true
                    }),
                    can_be_false: arguments.iter().all(|argument| {
                        cfg_possibility_when_test_is_disabled(argument).can_be_false
                    }),
                },
                "not" if arguments.len() == 1 => {
                    let argument = cfg_possibility_when_test_is_disabled(
                        arguments.first().expect("one not() argument"),
                    );
                    CfgPossibility {
                        can_be_true: argument.can_be_false,
                        can_be_false: argument.can_be_true,
                    }
                }
                _ => UNKNOWN_CFG,
            }
        }
    }
}

fn test_only_included_source_paths(src: &Path, sources: &[(PathBuf, String)]) -> HashSet<PathBuf> {
    let source_paths = sources
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<HashSet<_>>();
    let mut test_only_includes = HashSet::new();
    let mut production_includes = HashSet::new();
    for (including_path, content) in sources {
        let file = syn::parse_file(content).expect("parse Rust source for test-only includes");
        let mut collector = IncludeReachabilityCollector::default();
        collector.visit_file(&file);
        collect_include_references(
            &collector.test_only_invocations,
            &mut test_only_includes,
            src,
            including_path,
            &source_paths,
        );
        collect_include_references(
            &collector.production_invocations,
            &mut production_includes,
            src,
            including_path,
            &source_paths,
        );
    }
    test_only_includes
        .difference(&production_includes)
        .cloned()
        .collect()
}

fn collect_include_references(
    invocations: &[String],
    included: &mut HashSet<PathBuf>,
    src: &Path,
    including_path: &Path,
    source_paths: &HashSet<PathBuf>,
) {
    for invocation in invocations {
        for candidate in source_paths {
            if include_invocation_references(invocation, src, including_path, candidate) {
                included.insert(candidate.clone());
            }
        }
    }
}

#[derive(Default)]
struct IncludeReachabilityCollector {
    inside_test_only_item: bool,
    test_only_invocations: Vec<String>,
    production_invocations: Vec<String>,
}

impl<'ast> Visit<'ast> for IncludeReachabilityCollector {
    fn visit_item(&mut self, node: &'ast Item) {
        let parent_reachability = self.inside_test_only_item;
        self.inside_test_only_item |= attributes_require_test(item_attributes(node));
        visit::visit_item(self, node);
        self.inside_test_only_item = parent_reachability;
    }

    fn visit_impl_item(&mut self, node: &'ast ImplItem) {
        let parent_reachability = self.inside_test_only_item;
        self.inside_test_only_item |= attributes_require_test(impl_item_attributes(node));
        visit::visit_impl_item(self, node);
        self.inside_test_only_item = parent_reachability;
    }

    fn visit_trait_item(&mut self, node: &'ast TraitItem) {
        let parent_reachability = self.inside_test_only_item;
        self.inside_test_only_item |= attributes_require_test(trait_item_attributes(node));
        visit::visit_trait_item(self, node);
        self.inside_test_only_item = parent_reachability;
    }

    fn visit_foreign_item(&mut self, node: &'ast ForeignItem) {
        let parent_reachability = self.inside_test_only_item;
        self.inside_test_only_item |= attributes_require_test(foreign_item_attributes(node));
        visit::visit_foreign_item(self, node);
        self.inside_test_only_item = parent_reachability;
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if node.path.is_ident("include") {
            let invocation = node.tokens.to_string();
            if self.inside_test_only_item {
                self.test_only_invocations.push(invocation);
            } else {
                self.production_invocations.push(invocation);
            }
        }
        visit::visit_macro(self, node);
    }
}

#[test]
fn included_source_is_skipped_only_when_every_reachable_include_is_test_only() {
    let src = PathBuf::from("/synthetic/src");
    let parent = src.join("parent.rs");
    let fixture = src.join("fixture.rs");
    let test_only_parent = r#"
mod support {
    #[cfg(all(test, target_os = "linux"))]
    mod tests {
        include!("fixture.rs");
    }
}
"#;
    let mut sources = vec![
        (parent, test_only_parent.to_owned()),
        (fixture.clone(), "const VENUE: &str = \"bybit\";".to_owned()),
    ];
    assert!(test_only_included_source_paths(&src, &sources).contains(&fixture));
    assert!(sample_venue_violations(&src, &sources).is_empty());

    sources[0].1.push_str(
        r#"
mod production {
    include!("fixture.rs");
}
"#,
    );
    assert!(!test_only_included_source_paths(&src, &sources).contains(&fixture));
    assert!(
        sample_venue_violations(&src, &sources)
            .iter()
            .any(|failure| failure.contains("fixture.rs") && failure.contains("bybit"))
    );
}

fn include_invocation_references(
    invocation: &str,
    src: &Path,
    including_path: &Path,
    candidate: &Path,
) -> bool {
    let mut literals = Vec::new();
    if let Ok(relative) = candidate.strip_prefix(src) {
        literals.push(format!("/src/{}", slash_path(relative)));
    }
    if let Some(parent) = including_path.parent()
        && let Ok(relative) = candidate.strip_prefix(parent)
    {
        literals.push(slash_path(relative));
    }
    literals
        .iter()
        .any(|literal| invocation.contains(&format!("\"{literal}\"")))
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn nested_cfg_test_items_are_excluded_but_production_reachable_items_are_not() {
    let nested_test_cfg = r#"
const PRODUCTION_BEFORE: &str = "synthetic";
fn borrow<'a>(value: &'a str) -> &'a str { value }
#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests {
    const TEST_VENUE: &str = "bybit";
}
const PRODUCTION_AFTER: &str = "synthetic";
"#;
    let production = production_source(nested_test_cfg);
    assert!(!production.contains("bybit"));
    assert!(production.contains("PRODUCTION_BEFORE"));
    assert!(production.contains("PRODUCTION_AFTER"));
    assert!(production.contains("borrow"));

    let production_reachable = nested_test_cfg.replacen("all(test", "any(test", 1);
    assert!(production_source(&production_reachable).contains("bybit"));
}

#[test]
fn cfg_test_include_is_excluded_only_while_its_parent_module_is_test_only() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = rust_sources(&src);
    let parent = src.join("source_universe_batch_execution.rs");
    let included = src.join("source_universe_batch_execution_tests.rs");
    assert!(test_only_included_source_paths(&src, &sources).contains(&included));
    assert!(sample_venue_violations(&src, &sources).is_empty());

    let parent_source = sources
        .iter_mut()
        .find_map(|(path, source)| (path == &parent).then_some(source))
        .expect("batch execution parent source");
    let test_gate = "#[cfg(test)]\nmod source_universe_batch_tests";
    assert_eq!(parent_source.matches(test_gate).count(), 1);
    *parent_source = parent_source.replacen(test_gate, "mod source_universe_batch_tests", 1);

    assert!(!test_only_included_source_paths(&src, &sources).contains(&included));
    let failures = sample_venue_violations(&src, &sources);
    for needle in ["bybit", "public_archive"] {
        assert!(
            failures.iter().any(|failure| {
                failure.contains("source_universe_batch_execution_tests.rs")
                    && failure.contains(needle)
            }),
            "making the include production-reachable must expose {needle:?}: {failures:?}"
        );
    }
}

fn needle_allowed_in_production_path(needle: &str, path: &Path, src: &Path) -> bool {
    let relative = path.strip_prefix(src).expect("source-relative path");
    let relative = relative.to_str().expect("UTF-8 source path");
    if relative == "retired_backfill_provenance.rs" {
        return matches!(needle, "binance" | "bybit" | "bnbusdc");
    }
    if relative == "reference_fixture_index.rs" {
        return matches!(needle, "binance" | "bybit" | "pmxt" | "polymarket");
    }
    if !matches!(needle, "pmxt" | "polymarket") {
        return false;
    }

    matches!(
        relative,
        "lib.rs"
            | "pmxt_one_off_backfill_projection.rs"
            | "polymarket_metadata_gate.rs"
            | "polymarket_nt_surface_proof.rs"
            | "bin/pmxt_one_off_l2_artifact_root_run.rs"
            | "bin/polymarket_metadata_gate.rs"
    )
}

#[test]
fn retired_backfill_provenance_allowlist_is_exact_and_path_scoped() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let provenance = src.join("retired_backfill_provenance.rs");
    let generic_runtime = src.join("retired_backfill_evidence.rs");

    for needle in ["binance", "bybit", "bnbusdc"] {
        assert!(needle_allowed_in_production_path(needle, &provenance, &src));
        assert!(!needle_allowed_in_production_path(
            needle,
            &generic_runtime,
            &src
        ));
    }
    for needle in ["pmxt", "polymarket", "public_archive"] {
        assert!(!needle_allowed_in_production_path(
            needle,
            &provenance,
            &src
        ));
    }
}

#[test]
fn reference_fixture_index_sample_allowlist_is_limited_to_provenance_terms() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let path = src.join("reference_fixture_index.rs");

    for needle in ["binance", "bybit", "pmxt", "polymarket"] {
        assert!(needle_allowed_in_production_path(needle, &path, &src));
    }
    for needle in [
        "bnbusdc",
        "public_archive",
        "upbit",
        "bithumb",
        "korbit",
        "coinone",
        "kimchi",
        "korean_spot",
        "reference_price",
        "fx_quote",
        "token_mapping",
    ] {
        assert!(!needle_allowed_in_production_path(needle, &path, &src));
    }
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    visit_rust_files(root, &mut files);
    files.sort();
    files
}

fn rust_sources(root: &Path) -> Vec<(PathBuf, String)> {
    rust_files(root)
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path).expect("read Rust source");
            (path, source)
        })
        .collect()
}

fn visit_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("read source entry").path();
        if path.is_dir() {
            visit_rust_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}
