use std::{
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
    let mut failures = Vec::new();
    for (path, content) in sources {
        let lower = production_source(content).to_ascii_lowercase();
        for needle in SAMPLE_VENUE_NEEDLES {
            if lower.contains(needle) && !needle_allowed_in_production_path(needle, path, src) {
                failures.push(format!("{} contains {needle:?}", path.display()));
            }
        }
        failures.extend(production_test_support_reference_violations(path, content));
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
        let function = rust_function_source(&source, function_name);
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

fn rust_function_source(source: &str, function_name: &str) -> String {
    let file = syn::parse_file(source).expect("parse Rust source for named function");
    let mut collector = NamedFunctionCollector {
        function_name,
        functions: Vec::new(),
    };
    collector.visit_file(&file);
    assert_eq!(
        collector.functions.len(),
        1,
        "expected exactly one Rust function named {function_name}"
    );
    collector.functions.pop().expect("one named function")
}

struct NamedFunctionCollector<'a> {
    function_name: &'a str,
    functions: Vec<String>,
}

impl<'ast> Visit<'ast> for NamedFunctionCollector<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if node.sig.ident == self.function_name {
            self.functions.push(node.to_token_stream().to_string());
        }
        visit::visit_item_fn(self, node);
    }
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

#[derive(Default)]
struct ProductionTestSupportReferenceCollector {
    references: Vec<&'static str>,
}

impl<'ast> Visit<'ast> for ProductionTestSupportReferenceCollector {
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if node.path.is_ident("include") && references_test_support(&node.tokens.to_string()) {
            self.references.push("include!");
        }
        visit::visit_macro(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if node.attrs.iter().any(|attribute| {
            attribute.path().is_ident("path")
                && match &attribute.meta {
                    Meta::NameValue(value) => match &value.value {
                        syn::Expr::Lit(value) => match &value.lit {
                            syn::Lit::Str(value) => references_test_support(&value.value()),
                            _ => false,
                        },
                        _ => false,
                    },
                    _ => false,
                }
        }) {
            self.references.push("#[path]");
        }
        visit::visit_item_mod(self, node);
    }
}

fn production_test_support_reference_violations(path: &Path, content: &str) -> Vec<String> {
    let mut file = syn::parse_file(content).expect("parse Rust source for test-support fence");
    let mut pruner = ProductionItemPruner;
    pruner.visit_file_mut(&mut file);
    let mut collector = ProductionTestSupportReferenceCollector::default();
    collector.visit_file(&file);
    collector
        .references
        .into_iter()
        .map(|kind| {
            format!(
                "{} contains production-reachable {kind} into tests/support",
                path.display()
            )
        })
        .collect()
}

fn references_test_support(value: &str) -> bool {
    let mut normalized = value.replace('\\', "/");
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized.retain(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-')
    });
    normalized.contains("tests/support/")
}

fn production_manifest_test_support_violations(manifest: &str) -> Vec<String> {
    // Cargo's implicit roots are build.rs, src/lib.rs, src/main.rs, and
    // src/bin/*.rs. Only explicit build/lib/bin path overrides can redirect a
    // production target into tests/support.
    let manifest = manifest
        .parse::<toml::Value>()
        .expect("parse backtesting crate Cargo.toml");
    let mut violations = Vec::new();
    if let Some(build) = manifest
        .get("package")
        .and_then(|package| package.get("build"))
        .and_then(toml::Value::as_str)
        && references_test_support(build)
    {
        violations.push("package.build".to_owned());
    }
    if let Some(path) = manifest
        .get("lib")
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
        && references_test_support(path)
    {
        violations.push("lib.path".to_owned());
    }
    if let Some(binaries) = manifest.get("bin").and_then(toml::Value::as_array) {
        for (index, binary) in binaries.iter().enumerate() {
            if binary
                .get("path")
                .and_then(toml::Value::as_str)
                .is_some_and(references_test_support)
            {
                violations.push(format!("bin[{index}].path"));
            }
        }
    }
    violations
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
fn test_support_include_is_allowed_only_while_its_parent_module_is_test_only() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_root.join("src");
    let mut sources = rust_sources(&src);
    let parent = src.join("source_universe_batch_execution.rs");
    assert!(
        !src.join("source_universe_batch_execution_tests.rs")
            .exists()
    );
    assert!(
        crate_root
            .join("tests/support/source_universe_batch_execution_tests.rs")
            .is_file()
    );
    assert!(sample_venue_violations(&src, &sources).is_empty());

    let parent_source = sources
        .iter_mut()
        .find_map(|(path, source)| (path == &parent).then_some(source))
        .expect("batch execution parent source");
    let test_gate = "#[cfg(test)]\nmod source_universe_batch_tests";
    assert_eq!(parent_source.matches(test_gate).count(), 1);
    *parent_source = parent_source.replacen(test_gate, "mod source_universe_batch_tests", 1);

    let failures = sample_venue_violations(&src, &sources);
    assert!(
        failures.iter().any(|failure| {
            failure.contains("source_universe_batch_execution.rs")
                && failure.contains("production-reachable include!")
        }),
        "making the test-support include production-reachable must fail: {failures:?}"
    );
}

#[test]
fn production_module_path_cannot_reintroduce_test_support_as_a_source_root() {
    let path = Path::new("/synthetic/src/production.rs");
    let production = r#"
#[path = "../tests/support/source_universe_batch_execution_tests.rs"]
mod leaked_test_support;
"#;
    let failures = production_test_support_reference_violations(path, production);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("production-reachable #[path]"));

    let test_only = format!("#[cfg(test)]\n{production}");
    assert!(production_test_support_reference_violations(path, &test_only).is_empty());

    let split_include = r#"
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/te", "sts/sup", "port/leak.rs"));
"#;
    assert_eq!(
        production_test_support_reference_violations(path, split_include).len(),
        1
    );
}

#[test]
fn cargo_production_targets_cannot_root_in_test_support() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("read backtesting crate manifest");
    assert!(production_manifest_test_support_violations(&manifest).is_empty());

    let mutated = format!(
        "{manifest}\n[[bin]]\nname = \"leaked-test-support\"\npath = \
         \"tests/support/source_universe_batch_execution_tests.rs\"\n"
    );
    let violations = production_manifest_test_support_violations(&mutated);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].starts_with("bin[") && violations[0].ends_with("].path"));
}

#[test]
fn named_function_lookup_ignores_braces_inside_literals_and_comments() {
    let source = r#"
fn guarded_boundary() {
    let misleading_brace = "}";
    /* }}} a textual brace must not terminate the AST item */
    let venue = "bybit";
}

fn neighboring_function() {
    let venue = "binance";
}
"#;
    let function = rust_function_source(source, "guarded_boundary");
    assert!(function.contains("bybit"));
    assert!(!function.contains("binance"));
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
