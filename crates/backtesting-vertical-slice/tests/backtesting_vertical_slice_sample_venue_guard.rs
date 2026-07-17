use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
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
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest =
        fs::read_to_string(crate_root.join("Cargo.toml")).expect("read backtesting crate manifest");
    let (sources, mut failures) = production_source_graph(crate_root, &manifest);
    failures.extend(sample_venue_violations(crate_root, &sources));

    assert!(
        failures.is_empty(),
        "sample venue/instrument values must stay in TOML reference fixtures, tests, or explicit one-off proof modules, not generic production Rust:\n{}",
        failures.join("\n")
    );
}

fn sample_venue_violations(crate_root: &Path, sources: &BTreeMap<PathBuf, String>) -> Vec<String> {
    let mut failures = Vec::new();
    for (path, content) in sources {
        let lower = production_source(content).to_ascii_lowercase();
        for needle in SAMPLE_VENUE_NEEDLES {
            if lower.contains(needle)
                && !needle_allowed_in_production_path(needle, path, crate_root)
            {
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

    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        block
            .stmts
            .retain(|statement| !attributes_require_test(statement_attributes(statement)));
        for statement in &mut block.stmts {
            visit_mut::visit_stmt_mut(self, statement);
        }
    }

    fn visit_expr_match_mut(&mut self, expression: &mut syn::ExprMatch) {
        expression
            .arms
            .retain(|arm| !attributes_require_test(&arm.attrs));
        visit_mut::visit_expr_match_mut(self, expression);
    }

    fn visit_item_enum_mut(&mut self, item: &mut syn::ItemEnum) {
        item.variants = std::mem::take(&mut item.variants)
            .into_iter()
            .filter(|variant| !attributes_require_test(&variant.attrs))
            .collect();
        for variant in &mut item.variants {
            prune_test_only_fields(&mut variant.fields);
        }
        visit_mut::visit_item_enum_mut(self, item);
    }

    fn visit_item_struct_mut(&mut self, item: &mut syn::ItemStruct) {
        prune_test_only_fields(&mut item.fields);
        visit_mut::visit_item_struct_mut(self, item);
    }

    fn visit_item_union_mut(&mut self, item: &mut syn::ItemUnion) {
        item.fields.named = std::mem::take(&mut item.fields.named)
            .into_iter()
            .filter(|field| !attributes_require_test(&field.attrs))
            .collect();
        visit_mut::visit_item_union_mut(self, item);
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

fn prune_test_only_fields(fields: &mut syn::Fields) {
    match fields {
        syn::Fields::Named(fields) => {
            fields.named = std::mem::take(&mut fields.named)
                .into_iter()
                .filter(|field| !attributes_require_test(&field.attrs))
                .collect();
        }
        syn::Fields::Unnamed(fields) => {
            fields.unnamed = std::mem::take(&mut fields.unnamed)
                .into_iter()
                .filter(|field| !attributes_require_test(&field.attrs))
                .collect();
        }
        syn::Fields::Unit => {}
    }
}

fn statement_attributes(statement: &syn::Stmt) -> &[Attribute] {
    match statement {
        syn::Stmt::Local(statement) => &statement.attrs,
        syn::Stmt::Item(item) => item_attributes(item),
        syn::Stmt::Expr(expression, _) => expression_attributes(expression),
        syn::Stmt::Macro(statement) => &statement.attrs,
    }
}

fn expression_attributes(expression: &syn::Expr) -> &[Attribute] {
    match expression {
        syn::Expr::Array(expression) => &expression.attrs,
        syn::Expr::Assign(expression) => &expression.attrs,
        syn::Expr::Async(expression) => &expression.attrs,
        syn::Expr::Await(expression) => &expression.attrs,
        syn::Expr::Binary(expression) => &expression.attrs,
        syn::Expr::Block(expression) => &expression.attrs,
        syn::Expr::Break(expression) => &expression.attrs,
        syn::Expr::Call(expression) => &expression.attrs,
        syn::Expr::Cast(expression) => &expression.attrs,
        syn::Expr::Closure(expression) => &expression.attrs,
        syn::Expr::Const(expression) => &expression.attrs,
        syn::Expr::Continue(expression) => &expression.attrs,
        syn::Expr::Field(expression) => &expression.attrs,
        syn::Expr::ForLoop(expression) => &expression.attrs,
        syn::Expr::Group(expression) => &expression.attrs,
        syn::Expr::If(expression) => &expression.attrs,
        syn::Expr::Index(expression) => &expression.attrs,
        syn::Expr::Infer(expression) => &expression.attrs,
        syn::Expr::Let(expression) => &expression.attrs,
        syn::Expr::Lit(expression) => &expression.attrs,
        syn::Expr::Loop(expression) => &expression.attrs,
        syn::Expr::Macro(expression) => &expression.attrs,
        syn::Expr::Match(expression) => &expression.attrs,
        syn::Expr::MethodCall(expression) => &expression.attrs,
        syn::Expr::Paren(expression) => &expression.attrs,
        syn::Expr::Path(expression) => &expression.attrs,
        syn::Expr::Range(expression) => &expression.attrs,
        syn::Expr::RawAddr(expression) => &expression.attrs,
        syn::Expr::Reference(expression) => &expression.attrs,
        syn::Expr::Repeat(expression) => &expression.attrs,
        syn::Expr::Return(expression) => &expression.attrs,
        syn::Expr::Struct(expression) => &expression.attrs,
        syn::Expr::Try(expression) => &expression.attrs,
        syn::Expr::TryBlock(expression) => &expression.attrs,
        syn::Expr::Tuple(expression) => &expression.attrs,
        syn::Expr::Unary(expression) => &expression.attrs,
        syn::Expr::Unsafe(expression) => &expression.attrs,
        syn::Expr::While(expression) => &expression.attrs,
        syn::Expr::Yield(expression) => &expression.attrs,
        _ => &[],
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

fn production_source_graph(
    crate_root: &Path,
    manifest_source: &str,
) -> (BTreeMap<PathBuf, String>, Vec<String>) {
    let mut graph = ProductionSourceGraph::new(crate_root);
    let roots = production_target_roots(crate_root, manifest_source);
    match roots {
        Ok(roots) => {
            for root in roots {
                let module_dir = root
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| crate_root.to_path_buf());
                graph.visit_file(&root, &module_dir, false, MacroAuthorities::default());
            }
        }
        Err(errors) => graph.errors.extend(errors),
    }
    (graph.sources, graph.errors)
}

struct ProductionSourceGraph {
    crate_root: PathBuf,
    sources: BTreeMap<PathBuf, String>,
    visited_contexts: BTreeSet<(PathBuf, PathBuf, bool, MacroAuthorities)>,
    errors: Vec<String>,
}

impl ProductionSourceGraph {
    fn new(crate_root: &Path) -> Self {
        let crate_root = fs::canonicalize(crate_root).unwrap_or_else(|error| {
            panic!(
                "canonicalize production crate root {}: {error}",
                crate_root.display()
            )
        });
        Self {
            crate_root,
            sources: BTreeMap::new(),
            visited_contexts: BTreeSet::new(),
            errors: Vec::new(),
        }
    }

    fn visit_file(
        &mut self,
        unresolved_path: &Path,
        unresolved_module_dir: &Path,
        inside_inline_context: bool,
        inherited_macros: MacroAuthorities,
    ) {
        let path = match fs::canonicalize(unresolved_path) {
            Ok(path) => path,
            Err(error) => {
                self.errors.push(format!(
                    "production source {} cannot be resolved: {error}",
                    unresolved_path.display()
                ));
                return;
            }
        };
        if !path.starts_with(&self.crate_root) {
            self.errors.push(format!(
                "production source {} escapes canonical crate root {}",
                path.display(),
                self.crate_root.display()
            ));
            return;
        }
        let module_dir = lexical_absolute(unresolved_module_dir, &self.crate_root);
        if !module_dir.starts_with(&self.crate_root) {
            self.errors.push(format!(
                "production module context {} escapes canonical crate root {}",
                module_dir.display(),
                self.crate_root.display()
            ));
            return;
        }
        let context = (
            path.clone(),
            module_dir.clone(),
            inside_inline_context,
            inherited_macros.clone(),
        );
        if !self.visited_contexts.insert(context) {
            return;
        }

        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                self.errors.push(format!(
                    "production source {} is not readable UTF-8 Rust: {error}",
                    path.display()
                ));
                return;
            }
        };
        let mut file = match syn::parse_file(&source) {
            Ok(file) => file,
            Err(error) => {
                self.errors.push(format!(
                    "production source {} does not parse as Rust: {error}",
                    path.display()
                ));
                return;
            }
        };
        if attributes_require_test(&file.attrs) {
            return;
        }
        let mut pruner = ProductionItemPruner;
        pruner.visit_file_mut(&mut file);
        self.sources.entry(path.clone()).or_insert(source);

        let mut modules = ProductionModuleCollector::new(
            &path,
            &module_dir,
            inside_inline_context,
            inherited_macros.clone(),
        );
        modules.visit_file(&file);
        for error in modules.errors {
            self.errors.push(format!("{}: {error}", path.display()));
        }
        for module in modules.references {
            let (target, explicit_path) = if let Some(explicit_path) = module.explicit_path {
                (module.path_base.join(explicit_path), true)
            } else {
                let flat = module.default_base.join(format!("{}.rs", module.ident));
                let nested = module.default_base.join(&module.ident).join("mod.rs");
                match (flat.is_file(), nested.is_file()) {
                    (true, false) => (flat, false),
                    (false, true) => (nested, false),
                    (true, true) => {
                        self.errors.push(format!(
                            "{} module {} has ambiguous production sources {} and {}",
                            path.display(),
                            module.ident,
                            flat.display(),
                            nested.display()
                        ));
                        continue;
                    }
                    (false, false) => {
                        self.errors.push(format!(
                            "{} module {} has no resolvable production source",
                            path.display(),
                            module.ident
                        ));
                        continue;
                    }
                }
            };
            let child_module_dir = if explicit_path {
                target
                    .parent()
                    .expect("explicit module source parent")
                    .to_path_buf()
            } else {
                module_directory_for_file(&target)
            };
            self.visit_file(&target, &child_module_dir, false, module.textual_macros);
        }

        let mut includes = ProductionIncludeCollector::new(
            inherited_macros,
            &path,
            &module_dir,
            inside_inline_context,
        );
        includes.visit_file(&file);
        for error in includes.errors {
            self.errors.push(format!("{}: {error}", path.display()));
        }
        for include in includes.references {
            match evaluate_include_path(&include.expression, &self.crate_root) {
                Ok(include_path) => {
                    let target = if include_path.is_absolute() {
                        include_path
                    } else {
                        path.parent()
                            .expect("production source parent")
                            .join(include_path)
                    };
                    self.visit_file(
                        &target,
                        target.parent().expect("included production source parent"),
                        false,
                        include.textual_macros,
                    );
                }
                Err(error) => self.errors.push(format!(
                    "{} contains unresolved production include!: {error}",
                    path.display()
                )),
            }
        }
    }
}

struct ProductionModuleReference {
    ident: String,
    explicit_path: Option<PathBuf>,
    path_base: PathBuf,
    default_base: PathBuf,
    textual_macros: MacroAuthorities,
}

struct ProductionModuleCollector<'a> {
    source_path: &'a Path,
    current_module_dir: PathBuf,
    inline_depth: usize,
    textual_macros: MacroAuthorities,
    references: Vec<ProductionModuleReference>,
    errors: Vec<String>,
}

impl<'a> ProductionModuleCollector<'a> {
    fn new(
        source_path: &'a Path,
        module_dir: &Path,
        inside_inline_context: bool,
        textual_macros: MacroAuthorities,
    ) -> Self {
        Self {
            source_path,
            current_module_dir: module_dir.to_path_buf(),
            inline_depth: usize::from(inside_inline_context),
            textual_macros,
            references: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn path_base(&self) -> PathBuf {
        if self.inline_depth == 0 {
            self.source_path
                .parent()
                .expect("production source parent")
                .to_path_buf()
        } else {
            self.current_module_dir.clone()
        }
    }
}

impl<'ast> Visit<'ast> for ProductionModuleCollector<'_> {
    fn visit_file(&mut self, file: &'ast syn::File) {
        register_item_use_authorities(&mut self.textual_macros, &file.items);
        visit::visit_file(self, file);
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let inherited = self.textual_macros.clone();
        register_statement_use_authorities(&mut self.textual_macros, &block.stmts);
        visit::visit_block(self, block);
        self.textual_macros = inherited;
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if !item.mac.path.is_ident("macro_rules") {
            return;
        }
        let Some(name) = &item.ident else {
            self.errors
                .push("macro_rules! definition has no declared name".to_owned());
            return;
        };
        if macro_definition_is_source_inert(&item.mac.tokens) {
            self.textual_macros
                .source_inert_macros
                .insert(name.to_string());
        } else {
            self.errors.push(format!(
                "macro_rules! {name} can generate a production module/include edge"
            ));
        }
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        let explicit_path = match module_path_attribute(module) {
            Ok(path) => path,
            Err(error) => {
                self.errors.push(format!(
                    "module {} has unresolved #[path]: {error}",
                    module.ident
                ));
                return;
            }
        };
        let path_base = self.path_base();
        if module.content.is_none() {
            self.references.push(ProductionModuleReference {
                ident: module.ident.to_string(),
                explicit_path,
                path_base,
                default_base: self.current_module_dir.clone(),
                textual_macros: self.textual_macros.child_module_scope(),
            });
            return;
        }

        let previous_module_dir = self.current_module_dir.clone();
        let inherited_macros = self.textual_macros.clone();
        self.textual_macros = self.textual_macros.child_module_scope();
        if let Some((_, items)) = &module.content {
            register_item_use_authorities(&mut self.textual_macros, items);
        }
        self.current_module_dir = explicit_path.map_or_else(
            || previous_module_dir.join(module.ident.to_string()),
            |path| path_base.join(path),
        );
        self.inline_depth += 1;
        visit::visit_item_mod(self, module);
        self.inline_depth -= 1;
        self.current_module_dir = previous_module_dir;
        self.textual_macros = inherited_macros;
    }
}

fn lexical_absolute(path: &Path, crate_root: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        crate_root.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(component) => normalized.push(component),
        }
    }
    normalized
}

fn module_directory_for_file(path: &Path) -> PathBuf {
    let parent = path.parent().expect("module source parent");
    if path.file_name().and_then(|name| name.to_str()) == Some("mod.rs") {
        parent.to_path_buf()
    } else {
        parent.join(
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .expect("UTF-8 module source stem"),
        )
    }
}

fn module_path_attribute(module: &syn::ItemMod) -> Result<Option<PathBuf>, String> {
    for attribute in &module.attrs {
        if attribute.path().is_ident("cfg_attr") && cfg_attr_can_set_production_path(attribute)? {
            return Err(
                "production-conditional #[path] is not a single source authority".to_owned(),
            );
        }
    }
    let mut paths = module
        .attrs
        .iter()
        .filter_map(|attribute| attribute.path().is_ident("path").then_some(&attribute.meta));
    let Some(meta) = paths.next() else {
        return Ok(None);
    };
    if paths.next().is_some() {
        return Err("multiple #[path] attributes".to_owned());
    }
    let Meta::NameValue(value) = meta else {
        return Err("#[path] is not a name-value attribute".to_owned());
    };
    let syn::Expr::Lit(value) = &value.value else {
        return Err("#[path] is not a string literal".to_owned());
    };
    let syn::Lit::Str(value) = &value.lit else {
        return Err("#[path] is not a string literal".to_owned());
    };
    Ok(Some(PathBuf::from(value.value())))
}

fn cfg_attr_can_set_production_path(attribute: &Attribute) -> Result<bool, String> {
    let arguments = attribute
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_err(|error| format!("cfg_attr arguments do not parse: {error}"))?;
    cfg_attr_arguments_can_set_production_path(&arguments)
}

fn cfg_attr_arguments_can_set_production_path(
    arguments: &Punctuated<Meta, Token![,]>,
) -> Result<bool, String> {
    let mut arguments = arguments.iter();
    let condition = arguments
        .next()
        .ok_or_else(|| "cfg_attr has no condition".to_owned())?;
    if !cfg_possibility_when_test_is_disabled(condition).can_be_true {
        return Ok(false);
    }
    for argument in arguments {
        match argument {
            Meta::NameValue(value) if value.path.is_ident("path") => return Ok(true),
            Meta::Path(path) if path.is_ident("path") => return Ok(true),
            Meta::List(list) if list.path.is_ident("path") => return Ok(true),
            Meta::List(list) if list.path.is_ident("cfg_attr") => {
                let nested = Punctuated::<Meta, Token![,]>::parse_terminated
                    .parse2(list.tokens.clone())
                    .map_err(|error| format!("nested cfg_attr arguments do not parse: {error}"))?;
                if cfg_attr_arguments_can_set_production_path(&nested)? {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
    Ok(false)
}

struct ProductionIncludeCollector {
    macros: MacroAuthorities,
    source_path: PathBuf,
    current_module_dir: PathBuf,
    inline_depth: usize,
    references: Vec<ProductionIncludeReference>,
    errors: Vec<String>,
}

struct ProductionIncludeReference {
    expression: syn::Expr,
    textual_macros: MacroAuthorities,
}

impl ProductionIncludeCollector {
    fn new(
        macros: MacroAuthorities,
        source_path: &Path,
        module_dir: &Path,
        inside_inline_context: bool,
    ) -> Self {
        Self {
            macros,
            source_path: source_path.to_path_buf(),
            current_module_dir: module_dir.to_path_buf(),
            inline_depth: usize::from(inside_inline_context),
            references: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn collect_include(&mut self, node: &syn::Macro) {
        match syn::parse2::<syn::Expr>(node.tokens.clone()) {
            Ok(expression) => self.references.push(ProductionIncludeReference {
                expression,
                textual_macros: self.macros.clone(),
            }),
            Err(error) => self
                .errors
                .push(format!("include! expression does not parse: {error}")),
        }
    }

    fn reject_source_capable_macro(&mut self, node: &syn::Macro) {
        let source_tokens = macro_tokens_can_name_source_edges(&node.tokens);
        let cfg_if = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "cfg_if");
        if source_tokens || cfg_if {
            self.errors.push(format!(
                "opaque macro {}! can generate a production module/include edge",
                node.path.to_token_stream()
            ));
        }
    }

    fn reject_opaque_item_or_statement_macro(&mut self, node: &syn::Macro) {
        if macro_path_is_audited_source_inert(&node.path, &self.macros) {
            self.reject_source_capable_macro(node);
            return;
        }
        self.errors.push(format!(
            "unresolved item/statement macro {}! is not an audited source-inert authority",
            node.path.to_token_stream()
        ));
    }

    fn path_base(&self) -> PathBuf {
        if self.inline_depth == 0 {
            self.source_path
                .parent()
                .expect("production include source parent")
                .to_path_buf()
        } else {
            self.current_module_dir.clone()
        }
    }
}

impl<'ast> Visit<'ast> for ProductionIncludeCollector {
    fn visit_file(&mut self, file: &'ast syn::File) {
        register_item_use_authorities(&mut self.macros, &file.items);
        visit::visit_file(self, file);
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let inherited = self.macros.clone();
        register_statement_use_authorities(&mut self.macros, &block.stmts);
        visit::visit_block(self, block);
        self.macros = inherited;
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if module.content.is_none() {
            return;
        }
        let explicit_path = match module_path_attribute(module) {
            Ok(path) => path,
            Err(_) => return,
        };
        let previous_module_dir = self.current_module_dir.clone();
        let inherited_macros = self.macros.clone();
        self.macros = self.macros.child_module_scope();
        if let Some((_, items)) = &module.content {
            register_item_use_authorities(&mut self.macros, items);
        }
        let path_base = self.path_base();
        self.current_module_dir = explicit_path.map_or_else(
            || previous_module_dir.join(module.ident.to_string()),
            |path| path_base.join(path),
        );
        self.inline_depth += 1;
        visit::visit_item_mod(self, module);
        self.inline_depth -= 1;
        self.current_module_dir = previous_module_dir;
        self.macros = inherited_macros;
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if macro_resolves_to_direct_include(&node.path, &self.macros) {
            self.collect_include(node);
        } else {
            self.reject_source_capable_macro(node);
        }
    }

    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        if node.mac.path.is_ident("macro_rules") {
            if let Some(name) = &node.ident
                && macro_definition_is_source_inert(&node.mac.tokens)
            {
                self.macros.source_inert_macros.insert(name.to_string());
            }
            return;
        }
        if macro_resolves_to_direct_include(&node.mac.path, &self.macros) {
            self.collect_include(&node.mac);
        } else {
            self.reject_opaque_item_or_statement_macro(&node.mac);
        }
    }

    fn visit_stmt_macro(&mut self, node: &'ast syn::StmtMacro) {
        if macro_resolves_to_direct_include(&node.mac.path, &self.macros) {
            self.collect_include(&node.mac);
        } else {
            self.reject_opaque_item_or_statement_macro(&node.mac);
        }
    }
}

#[derive(Clone, Default, Eq, Ord, PartialEq, PartialOrd)]
struct MacroAuthorities {
    source_inert_macros: BTreeSet<String>,
    imported_macro_paths: BTreeMap<String, BTreeSet<Vec<String>>>,
    renamed_path_prefixes: BTreeMap<String, BTreeSet<Vec<String>>>,
    has_glob_import: bool,
}

impl MacroAuthorities {
    fn child_module_scope(&self) -> Self {
        Self {
            source_inert_macros: self.source_inert_macros.clone(),
            imported_macro_paths: BTreeMap::new(),
            renamed_path_prefixes: BTreeMap::new(),
            has_glob_import: false,
        }
    }
}

fn macro_path_is_audited_source_inert(path: &syn::Path, macros: &MacroAuthorities) -> bool {
    let Some(segments) = resolved_macro_segments(path, macros) else {
        return false;
    };
    if let [name] = segments.as_slice()
        && macros.source_inert_macros.contains(name)
    {
        return true;
    }
    macro_segments_are_audited_source_inert(&segments, macros)
}

fn macro_resolves_to_direct_include(path: &syn::Path, macros: &MacroAuthorities) -> bool {
    let raw = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    if let [name] = raw.as_slice()
        && macros.source_inert_macros.contains(name)
    {
        return false;
    }
    resolved_macro_segments(path, macros).is_some_and(|segments| {
        matches!(segments.as_slice(), [include] if include == "include")
            || matches!(segments.as_slice(), [namespace, include]
                if matches!(namespace.as_str(), "core" | "std") && include == "include")
    })
}

fn resolved_macro_segments(path: &syn::Path, macros: &MacroAuthorities) -> Option<Vec<String>> {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    let first = segments.first()?;
    let imports = if segments.len() == 1 {
        macros.imported_macro_paths.get(first)
    } else {
        macros.renamed_path_prefixes.get(first)
    };
    if let Some(imports) = imports {
        if imports.len() != 1 {
            return None;
        }
        let mut imported = imports.iter().next()?.clone();
        imported.extend(segments.into_iter().skip(1));
        return Some(imported);
    }
    if segments.len() == 1 && macros.has_glob_import {
        return None;
    }
    Some(segments)
}

fn macro_segments_are_audited_source_inert(segments: &[String], macros: &MacroAuthorities) -> bool {
    if let [name] = segments
        && macros.source_inert_macros.contains(name)
    {
        return true;
    }
    matches!(segments, [name] if builtin_source_inert_macro(name))
        || matches!(
            segments,
            [authority, name]
                if matches!(authority.as_str(), "core" | "std")
                    && builtin_source_inert_macro(name)
        )
        || matches!(
            segments,
            [authority, name]
                if matches!(
                    (authority.as_str(), name.as_str()),
                    ("anyhow", "anyhow" | "bail" | "ensure")
                        | ("serde_json", "json")
                        | ("tokio", "pin" | "select")
                        | ("nautilus_trading", "nautilus_strategy")
                )
        )
}

fn builtin_source_inert_macro(name: &str) -> bool {
    matches!(
        name,
        "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "debug_assert_eq"
            | "debug_assert_ne"
            | "eprint"
            | "eprintln"
            | "format"
            | "matches"
            | "panic"
            | "print"
            | "println"
            | "todo"
            | "unimplemented"
            | "unreachable"
            | "vec"
            | "write"
            | "writeln"
    )
}

fn register_item_use_authorities(macros: &mut MacroAuthorities, items: &[Item]) {
    for item in items {
        if let Item::Use(import) = item {
            register_use_tree(macros, Vec::new(), &import.tree);
        }
    }
}

fn register_statement_use_authorities(macros: &mut MacroAuthorities, statements: &[syn::Stmt]) {
    for statement in statements {
        if let syn::Stmt::Item(Item::Use(import)) = statement {
            register_use_tree(macros, Vec::new(), &import.tree);
        }
    }
}

fn register_use_tree(macros: &mut MacroAuthorities, mut prefix: Vec<String>, tree: &syn::UseTree) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            register_use_tree(macros, prefix, &path.tree);
        }
        syn::UseTree::Name(name) => {
            let imported = name.ident.to_string();
            if imported == "self" {
                if let Some(alias) = prefix.last().cloned() {
                    macros
                        .imported_macro_paths
                        .entry(alias)
                        .or_default()
                        .insert(prefix);
                }
            } else {
                prefix.push(imported.clone());
                macros
                    .imported_macro_paths
                    .entry(imported)
                    .or_default()
                    .insert(prefix);
            }
        }
        syn::UseTree::Rename(rename) => {
            prefix.push(rename.ident.to_string());
            macros
                .renamed_path_prefixes
                .entry(rename.rename.to_string())
                .or_default()
                .insert(prefix.clone());
            macros
                .imported_macro_paths
                .entry(rename.rename.to_string())
                .or_default()
                .insert(prefix);
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                register_use_tree(macros, prefix.clone(), item);
            }
        }
        syn::UseTree::Glob(_) => macros.has_glob_import = true,
    }
}

fn macro_definition_is_source_inert(tokens: &TokenStream) -> bool {
    let rendered = tokens.to_string();
    !macro_tokens_can_name_source_edges(tokens)
        && !token_stream_contains_bang(tokens)
        && ![": item", ": tt", ": stmt", ": block", ": expr", ": meta"]
            .iter()
            .any(|fragment| rendered.contains(fragment))
}

fn macro_tokens_can_name_source_edges(tokens: &TokenStream) -> bool {
    let tokens = tokens.clone().into_iter().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Ident(ident) if ident == "mod" => return true,
            TokenTree::Ident(ident) if matches!(ident.to_string().as_str(), "include" | "cfg_if") => {
                if matches!(tokens.get(index + 1), Some(TokenTree::Punct(punct)) if punct.as_char() == '!') {
                    return true;
                }
            }
            TokenTree::Punct(punct) if punct.as_char() == '#' => {
                if let Some(TokenTree::Group(group)) = tokens.get(index + 1)
                    && group.delimiter() == Delimiter::Bracket
                    && group.stream().into_iter().any(
                        |token| matches!(token, TokenTree::Ident(ident) if ident == "path" || ident == "cfg_attr"),
                    )
                {
                    return true;
                }
            }
            TokenTree::Group(group) if macro_tokens_can_name_source_edges(&group.stream()) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn token_stream_contains_bang(tokens: &TokenStream) -> bool {
    tokens.clone().into_iter().any(|token| match token {
        TokenTree::Punct(punct) => punct.as_char() == '!',
        TokenTree::Group(group) => token_stream_contains_bang(&group.stream()),
        TokenTree::Ident(_) | TokenTree::Literal(_) => false,
    })
}

fn evaluate_include_path(expression: &syn::Expr, crate_root: &Path) -> Result<PathBuf, String> {
    evaluate_string_expression(expression, crate_root).map(PathBuf::from)
}

fn evaluate_string_expression(expression: &syn::Expr, crate_root: &Path) -> Result<String, String> {
    match expression {
        syn::Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Str(value) => Ok(value.value()),
            _ => Err("include! path is not a string literal".to_owned()),
        },
        syn::Expr::Macro(expression) if expression.mac.path.is_ident("concat") => {
            let arguments = Punctuated::<syn::Expr, Token![,]>::parse_terminated
                .parse2(expression.mac.tokens.clone())
                .map_err(|error| format!("concat! arguments do not parse: {error}"))?;
            let mut value = String::new();
            for argument in arguments {
                value.push_str(&evaluate_string_expression(&argument, crate_root)?);
            }
            Ok(value)
        }
        syn::Expr::Macro(expression) if expression.mac.path.is_ident("env") => {
            let variable = syn::parse2::<syn::LitStr>(expression.mac.tokens.clone())
                .map_err(|error| format!("env! argument does not parse: {error}"))?;
            if variable.value() == "CARGO_MANIFEST_DIR" {
                Ok(crate_root.to_string_lossy().into_owned())
            } else {
                Err(format!(
                    "env! variable {:?} is not a stable source-root authority",
                    variable.value()
                ))
            }
        }
        _ => Err(format!(
            "dynamic expression {:?} is not allowed",
            expression.to_token_stream().to_string()
        )),
    }
}

fn production_target_roots(
    crate_root: &Path,
    manifest_source: &str,
) -> Result<Vec<PathBuf>, Vec<String>> {
    let manifest = match manifest_source.parse::<toml::Value>() {
        Ok(manifest) => manifest,
        Err(error) => return Err(vec![format!("Cargo.toml does not parse: {error}")]),
    };
    let package = manifest.get("package");
    let mut roots = BTreeSet::new();
    let mut errors = Vec::new();

    match package.and_then(|package| package.get("build")) {
        Some(toml::Value::Boolean(false)) => {}
        Some(toml::Value::Boolean(true)) | None => {
            add_if_file(&mut roots, crate_root.join("build.rs"));
        }
        Some(toml::Value::String(path)) => {
            roots.insert(crate_root.join(path));
        }
        Some(_) => errors.push("package.build must be a bool or path string".to_owned()),
    }

    match cargo_auto_setting(package, "autolib", true) {
        Ok(true) => {
            if let Some(path) = manifest
                .get("lib")
                .and_then(|lib| lib.get("path"))
                .and_then(toml::Value::as_str)
            {
                roots.insert(crate_root.join(path));
            } else {
                add_if_file(&mut roots, crate_root.join("src/lib.rs"));
            }
        }
        Ok(false) => {
            if let Some(lib) = manifest.get("lib") {
                let path = lib.get("path").and_then(toml::Value::as_str).map_or_else(
                    || crate_root.join("src/lib.rs"),
                    |path| crate_root.join(path),
                );
                roots.insert(path);
            }
        }
        Err(error) => errors.push(error),
    }

    add_target_roots(
        crate_root,
        &manifest,
        package,
        "bin",
        "autobins",
        "src/bin",
        Some("src/main.rs"),
        &mut roots,
        &mut errors,
    );
    add_target_roots(
        crate_root,
        &manifest,
        package,
        "example",
        "autoexamples",
        "examples",
        None,
        &mut roots,
        &mut errors,
    );
    add_target_roots(
        crate_root,
        &manifest,
        package,
        "bench",
        "autobenches",
        "benches",
        None,
        &mut roots,
        &mut errors,
    );

    if roots.is_empty() {
        errors.push("Cargo.toml declares no resolvable production source roots".to_owned());
    }
    if errors.is_empty() {
        Ok(roots.into_iter().collect())
    } else {
        Err(errors)
    }
}

fn cargo_auto_setting(
    package: Option<&toml::Value>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match package.and_then(|package| package.get(key)) {
        None => Ok(default),
        Some(toml::Value::Boolean(value)) => Ok(*value),
        Some(_) => Err(format!("package.{key} must be a bool")),
    }
}

#[allow(clippy::too_many_arguments)]
fn add_target_roots(
    crate_root: &Path,
    manifest: &toml::Value,
    package: Option<&toml::Value>,
    table: &str,
    auto_key: &str,
    directory: &str,
    singleton: Option<&str>,
    roots: &mut BTreeSet<PathBuf>,
    errors: &mut Vec<String>,
) {
    match cargo_auto_setting(package, auto_key, true) {
        Ok(true) => {
            if let Some(singleton) = singleton {
                add_if_file(roots, crate_root.join(singleton));
            }
            discover_automatic_target_roots(&crate_root.join(directory), roots, errors);
        }
        Ok(false) => {}
        Err(error) => errors.push(error),
    }

    let Some(targets) = manifest.get(table) else {
        return;
    };
    let Some(targets) = targets.as_array() else {
        errors.push(format!("[[{table}]] must be an array of target tables"));
        return;
    };
    for (index, target) in targets.iter().enumerate() {
        let Some(target) = target.as_table() else {
            errors.push(format!("{table}[{index}] is not a target table"));
            continue;
        };
        if let Some(path) = target.get("path").and_then(toml::Value::as_str) {
            roots.insert(crate_root.join(path));
            continue;
        }
        let Some(name) = target.get("name").and_then(toml::Value::as_str) else {
            errors.push(format!("{table}[{index}] needs a path or name"));
            continue;
        };
        if table == "bin"
            && singleton.is_some_and(|singleton| crate_root.join(singleton).is_file())
            && package
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                == Some(name)
        {
            roots.insert(crate_root.join(singleton.expect("bin singleton")));
            continue;
        }
        let base = crate_root.join(directory);
        let flat = base.join(format!("{name}.rs"));
        let nested = base.join(name).join("main.rs");
        match (flat.is_file(), nested.is_file()) {
            (true, false) => {
                roots.insert(flat);
            }
            (false, true) => {
                roots.insert(nested);
            }
            _ => errors.push(format!(
                "{table}[{index}] has no unique implicit source for target {name:?}"
            )),
        }
    }
}

fn discover_automatic_target_roots(
    directory: &Path,
    roots: &mut BTreeSet<PathBuf>,
    errors: &mut Vec<String>,
) {
    if !directory.exists() {
        return;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!(
                "automatic target directory {} is unreadable: {error}",
                directory.display()
            ));
            return;
        }
    };
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                errors.push(format!(
                    "automatic target directory {} has an unreadable entry: {error}",
                    directory.display()
                ));
                continue;
            }
        };
        if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            roots.insert(path);
        } else if path.is_dir() {
            add_if_file(roots, path.join("main.rs"));
        }
    }
}

fn add_if_file(roots: &mut BTreeSet<PathBuf>, path: PathBuf) {
    if path.is_file() {
        roots.insert(path);
    }
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
fn cfg_test_arms_variants_and_fields_are_pruned_without_hiding_production_reachable_values() {
    let cfg_test_only = r#"
struct NamedFields {
    production: u64,
    #[cfg(test)]
    bybit_field: u64,
}

#[cfg(test)]
type BybitTupleField = u64;

struct TupleFields(
    u64,
    #[cfg(test)]
    BybitTupleField,
);

union UnionFields {
    production: u64,
    #[cfg(test)]
    public_archive_field: u64,
}

enum Variants {
    Production,
    #[cfg(test)]
    Binance,
    WithFields {
        production: u64,
        #[cfg(test)]
        bnbusdc_field: u64,
    },
}

fn classify(value: u8) -> &'static str {
    match value {
        #[cfg(test)]
        0 => "polymarket",
        _ => "synthetic",
    }
}
"#;
    let production = production_source(cfg_test_only).to_ascii_lowercase();
    for test_only in [
        "bybit_field",
        "bybittuplefield",
        "public_archive_field",
        "binance",
        "bnbusdc_field",
        "polymarket",
    ] {
        assert!(
            !production.contains(test_only),
            "cfg(test)-only {test_only} must be absent from the production AST: {production}"
        );
    }
    for production_value in ["production", "synthetic", "classify"] {
        assert!(production.contains(production_value));
    }

    let production_reachable = cfg_test_only.replace("cfg(test)", "cfg(any(test, unix))");
    let production_reachable = production_source(&production_reachable).to_ascii_lowercase();
    for reachable in [
        "bybit_field",
        "bybittuplefield",
        "public_archive_field",
        "binance",
        "bnbusdc_field",
        "polymarket",
    ] {
        assert!(
            production_reachable.contains(reachable),
            "production-reachable {reachable} must not be pruned: {production_reachable}"
        );
    }
}

#[test]
fn test_support_include_is_allowed_only_while_its_parent_module_is_test_only() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(crate_root.join("Cargo.toml")).expect("read manifest");
    let (sources, errors) = production_source_graph(crate_root, &manifest);
    assert!(errors.is_empty(), "actual source graph errors: {errors:?}");
    assert!(
        !crate_root
            .join("src/source_universe_batch_execution_tests.rs")
            .exists()
    );
    let test_support =
        fs::canonicalize(crate_root.join("tests/support/source_universe_batch_execution_tests.rs"))
            .expect("canonical test support");
    assert!(!sources.contains_key(&test_support));

    let fixture = tempfile::tempdir().expect("temporary crate");
    let synthetic_root = fixture.path().join("crate");
    write_synthetic_source(
        &synthetic_root,
        "src/lib.rs",
        r#"
#[cfg(test)]
mod tests {
    include!("../tests/support/leak.rs");
}
"#,
    );
    write_synthetic_source(
        &synthetic_root,
        "tests/support/leak.rs",
        "const TEST_VENUE: &str = \"bybit\";\n",
    );
    let manifest = synthetic_manifest();
    let (sources, errors) = production_source_graph(&synthetic_root, &manifest);
    assert!(errors.is_empty(), "test-only include errors: {errors:?}");
    assert!(sample_venue_violations(&synthetic_root, &sources).is_empty());

    write_synthetic_source(
        &synthetic_root,
        "src/lib.rs",
        r#"include!("../tests/./support/leak.rs");"#,
    );
    let (sources, errors) = production_source_graph(&synthetic_root, &manifest);
    assert!(errors.is_empty(), "production include errors: {errors:?}");
    let failures = sample_venue_violations(&synthetic_root, &sources);
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("leak.rs") && failure.contains("bybit")),
        "canonical production reachability through tests/./support must fail: {failures:?}"
    );
}

#[test]
fn production_module_paths_and_includes_scan_their_canonical_targets() {
    let fixture = tempfile::tempdir().expect("temporary crate");
    let crate_root = fixture.path().join("crate");
    write_synthetic_source(
        &crate_root,
        "tests/fixtures/venue_leak.rs",
        "const LEAKED_VENUE: &str = \"binance\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
#[path = "../tests/fixtures/venue_leak.rs"]
mod leaked_fixture;
"#,
    );
    let manifest = synthetic_manifest();
    let (sources, errors) = production_source_graph(&crate_root, &manifest);
    assert!(errors.is_empty(), "#[path] graph errors: {errors:?}");
    let failures = sample_venue_violations(&crate_root, &sources);
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("venue_leak.rs") && failure.contains("binance")),
        "production #[path] target must be scanned: {failures:?}"
    );

    write_synthetic_source(
        &crate_root,
        "tests/support/path_leak.rs",
        "const LEAKED_VENUE: &str = \"bybit\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
#[path = "../tests/./support/path_leak.rs"]
mod leaked_support;
"#,
    );
    let (sources, errors) = production_source_graph(&crate_root, &manifest);
    assert!(errors.is_empty(), "dotted #[path] graph errors: {errors:?}");
    assert!(
        sample_venue_violations(&crate_root, &sources)
            .iter()
            .any(|failure| failure.contains("path_leak.rs") && failure.contains("bybit")),
        "canonical production reachability through tests/./support must fail"
    );

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/te", "sts/fixtures/venue_leak.rs"));
"#,
    );
    let (sources, errors) = production_source_graph(&crate_root, &manifest);
    assert!(errors.is_empty(), "split include graph errors: {errors:?}");
    assert!(
        sample_venue_violations(&crate_root, &sources)
            .iter()
            .any(|failure| failure.contains("venue_leak.rs") && failure.contains("binance"))
    );

    write_synthetic_source(&crate_root, "src/lib.rs", "include!(runtime_path());\n");
    let (_, errors) = production_source_graph(&crate_root, &manifest);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("unresolved production include!")),
        "dynamic include must fail closed: {errors:?}"
    );
}

#[test]
fn path_resolution_uses_the_compiler_source_base_and_explicit_children_are_mod_like() {
    let fixture = tempfile::tempdir().expect("temporary crate");
    let crate_root = fixture.path().join("crate");
    write_synthetic_source(&crate_root, "src/lib.rs", "mod outer;\n");
    write_synthetic_source(
        &crate_root,
        "src/outer.rs",
        "#[path = \"actual.rs\"] mod child;\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/outer/actual.rs",
        "const DECOY: &str = \"synthetic\";\n",
    );
    write_synthetic_source(&crate_root, "src/actual.rs", "mod nested;\n");
    write_synthetic_source(
        &crate_root,
        "src/actual/nested.rs",
        "const DECOY: &str = \"synthetic\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/nested.rs",
        "const LEAKED_VENUE: &str = \"bybit\";\n",
    );

    let (sources, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(errors.is_empty(), "compiler path graph errors: {errors:?}");
    let failures = sample_venue_violations(&crate_root, &sources);
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("nested.rs") && failure.contains("bybit")),
        "source-parent #[path] and mod-like explicit children must reach the real leak: {failures:?}"
    );
    assert!(!sources.contains_key(
        &fs::canonicalize(crate_root.join("src/outer/actual.rs")).expect("canonical decoy")
    ));
}

#[test]
fn inline_include_resets_child_resolution_to_the_included_file_parent() {
    let fixture = tempfile::tempdir().expect("temporary crate");
    let crate_root = fixture.path().join("crate");
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
mod inline_scope {
    include!("included.rs");
}
"#,
    );
    write_synthetic_source(&crate_root, "src/included.rs", "mod child;\n");
    write_synthetic_source(
        &crate_root,
        "src/child.rs",
        "const LEAKED_VENUE: &str = \"bybit\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/inline_scope/child.rs",
        "const INLINE_CONTEXT_DECOY: &str = \"synthetic\";\n",
    );

    let (sources, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(errors.is_empty(), "inline include graph errors: {errors:?}");
    let failures = sample_venue_violations(&crate_root, &sources);
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("child.rs") && failure.contains("bybit")),
        "included child must resolve from the included file parent: {failures:?}"
    );
    assert!(
        !sources.contains_key(
            &fs::canonicalize(crate_root.join("src/inline_scope/child.rs"))
                .expect("canonical inline-context decoy")
        )
    );
}

#[test]
fn block_local_module_items_are_part_of_the_production_source_graph() {
    let fixture = tempfile::tempdir().expect("temporary crate");
    let crate_root = fixture.path().join("crate");
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
fn default_module() { mod block_default; }
fn attributed_module() {
    #[path = "../tests/fixtures/block_path.rs"]
    mod block_path;
}
"#,
    );
    write_synthetic_source(
        &crate_root,
        "src/block_default.rs",
        "const LEAKED_VENUE: &str = \"binance\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "tests/fixtures/block_path.rs",
        "const LEAKED_SOURCE: &str = \"public_archive\";\n",
    );
    let (sources, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.is_empty(),
        "block-local module graph errors: {errors:?}"
    );
    let failures = sample_venue_violations(&crate_root, &sources);
    for (path, needle) in [
        ("block_default.rs", "binance"),
        ("block_path.rs", "public_archive"),
    ] {
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains(path) && failure.contains(needle)),
            "block-local module {path} must be scanned for {needle}: {failures:?}"
        );
    }
}

#[test]
fn qualified_includes_and_opaque_source_macros_fail_closed() {
    let fixture = tempfile::tempdir().expect("temporary crate");
    let crate_root = fixture.path().join("crate");
    write_synthetic_source(
        &crate_root,
        "tests/fixtures/core.rs",
        "const LEAKED_VENUE: &str = \"binance\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "tests/fixtures/std.rs",
        "const LEAKED_VENUE: &str = \"bybit\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
core::include!("../tests/fixtures/core.rs");
std::include!("../tests/fixtures/std.rs");
"#,
    );
    let (sources, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(errors.is_empty(), "qualified include errors: {errors:?}");
    let failures = sample_venue_violations(&crate_root, &sources);
    assert!(failures.iter().any(|failure| failure.contains("core.rs")));
    assert!(failures.iter().any(|failure| failure.contains("std.rs")));

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
macro_rules! inject_source { () => { include!("../tests/fixtures/core.rs"); } }
inject_source!();
"#,
    );
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.iter().any(|error| {
            error.contains("macro_rules! inject_source")
                && error.contains("production module/include edge")
        }),
        "macro_rules source indirection must fail closed: {errors:?}"
    );

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
macro_rules! inherited_source { () => { include!("../tests/fixtures/core.rs"); } }
mod child;
"#,
    );
    write_synthetic_source(&crate_root, "src/child.rs", "inherited_source!();\n");
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.iter().any(|error| {
            error.contains("macro_rules! inherited_source")
                && error.contains("production module/include edge")
        }),
        "source-capable macro_rules authority must fail at its definition before textual scope crosses into a child file: {errors:?}"
    );

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
macro_rules! inherited_inert { () => { const SYNTHETIC: &str = "synthetic"; } }
mod child;
"#,
    );
    write_synthetic_source(&crate_root, "src/child.rs", "inherited_inert!();\n");
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.is_empty(),
        "proven source-inert textual macro authority must cross into its child module without a false positive: {errors:?}"
    );

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
use std::include as ensure;
ensure!("../tests/fixtures/core.rs");
"#,
    );
    let (sources, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.is_empty(),
        "the include! macro-namespace alias must resolve to its built-in authority: {errors:?}"
    );
    assert!(
        sample_venue_violations(&crate_root, &sources)
            .iter()
            .any(|failure| failure.contains("core.rs") && failure.contains("binance")),
        "an include! alias named like an inert macro must still traverse its source edge"
    );

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
fn checked_control() {
    use std::assert as checked;
    checked!(true);
}
"#,
    );
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.is_empty(),
        "a block-scoped alias to an audited source-inert macro must remain accepted: {errors:?}"
    );

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        "cfg_if::cfg_if! { if #[cfg(unix)] { mod hidden; } }\n",
    );
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.iter().any(|error| {
            error.contains("cfg_if") && error.contains("not an audited source-inert authority")
        }),
        "cfg_if source indirection must fail closed: {errors:?}"
    );

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
macro_rules! external_item_shape { ($name:ident) => { struct $name; } }
external_item_shape!(SyntheticStrategy);
"#,
    );
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors.is_empty(),
        "a defined source-inert item macro must remain accepted: {errors:?}"
    );
}

#[test]
fn nested_cfg_attr_paths_fail_closed_and_test_only_sources_are_pruned() {
    let fixture = tempfile::tempdir().expect("temporary crate");
    let crate_root = fixture.path().join("crate");
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
#[cfg_attr(unix, cfg_attr(unix, path = "../tests/fixtures/leak.rs"))]
mod conditional;
"#,
    );
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("production-conditional #[path]")),
        "nested production cfg_attr(path) must fail closed: {errors:?}"
    );

    write_synthetic_source(
        &crate_root,
        "tests/fixtures/leak.rs",
        "const TEST_VENUE: &str = \"bybit\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        "#![cfg(test)]\nconst TEST_VENUE: &str = \"bybit\";\n",
    );
    let (sources, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(errors.is_empty(), "inner cfg(test) errors: {errors:?}");
    assert!(sources.is_empty(), "test-only file must not be production");

    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        r#"
fn guarded_statement() {
    #[cfg(test)]
    include!("../tests/fixtures/leak.rs");
}
"#,
    );
    let (sources, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(errors.is_empty(), "cfg(test) statement errors: {errors:?}");
    assert!(sample_venue_violations(&crate_root, &sources).is_empty());
}

#[test]
fn every_cargo_production_target_root_is_scanned() {
    let fixture = tempfile::tempdir().expect("temporary crate");
    let crate_root = fixture.path().join("crate");
    for (path, venue) in [
        ("runtime/lib.rs", "binance"),
        ("runtime/bin.rs", "bybit"),
        ("runtime/build.rs", "public_archive"),
        ("runtime/example.rs", "upbit"),
        ("runtime/bench.rs", "bithumb"),
    ] {
        write_synthetic_source(
            &crate_root,
            path,
            &format!("const LEAKED_VALUE: &str = {venue:?};\n"),
        );
    }
    let manifest = r#"
[package]
name = "production-root-guard"
version = "0.0.0"
edition = "2024"
autobins = false
autoexamples = false
autobenches = false
build = "runtime/build.rs"

[lib]
path = "runtime/lib.rs"

[[bin]]
name = "guard-bin"
path = "runtime/bin.rs"

[[example]]
name = "guard-example"
path = "runtime/example.rs"

[[bench]]
name = "guard-bench"
path = "runtime/bench.rs"
"#;
    let (sources, errors) = production_source_graph(&crate_root, manifest);
    assert!(
        errors.is_empty(),
        "explicit target graph errors: {errors:?}"
    );
    let failures = sample_venue_violations(&crate_root, &sources);
    for (path, venue) in [
        ("lib.rs", "binance"),
        ("bin.rs", "bybit"),
        ("build.rs", "public_archive"),
        ("example.rs", "upbit"),
        ("bench.rs", "bithumb"),
    ] {
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains(path) && failure.contains(venue)),
            "production target {path} must be scanned for {venue}: {failures:?}"
        );
    }
}

#[test]
fn production_source_graph_rejects_canonical_escape() {
    let fixture = tempfile::tempdir().expect("temporary source graph");
    let crate_root = fixture.path().join("crate");
    write_synthetic_source(
        fixture.path(),
        "outside.rs",
        "const SYNTHETIC: &str = \"synthetic\";\n",
    );
    write_synthetic_source(
        &crate_root,
        "src/lib.rs",
        "include!(\"../../outside.rs\");\n",
    );
    let (_, errors) = production_source_graph(&crate_root, &synthetic_manifest());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("escapes canonical crate root")),
        "canonical source escape must fail closed: {errors:?}"
    );
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

fn needle_allowed_in_production_path(needle: &str, path: &Path, crate_root: &Path) -> bool {
    let crate_root = fs::canonicalize(crate_root).unwrap_or_else(|_| crate_root.to_path_buf());
    let relative = path
        .strip_prefix(&crate_root)
        .expect("canonical crate-relative path");
    let relative = relative.to_str().expect("UTF-8 source path");
    if relative == "src/retired_backfill_provenance.rs" {
        return matches!(needle, "binance" | "bybit" | "bnbusdc");
    }
    if relative == "src/reference_fixture_index.rs" {
        return matches!(needle, "binance" | "bybit" | "pmxt" | "polymarket");
    }
    if !matches!(needle, "pmxt" | "polymarket") {
        return false;
    }

    matches!(
        relative,
        "src/lib.rs"
            | "src/pmxt_one_off_backfill_projection.rs"
            | "src/polymarket_metadata_gate.rs"
            | "src/polymarket_nt_surface_proof.rs"
            | "src/bin/pmxt_one_off_l2_artifact_root_run.rs"
            | "src/bin/polymarket_metadata_gate.rs"
    )
}

#[test]
fn retired_backfill_provenance_allowlist_is_exact_and_path_scoped() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_root.join("src");
    let provenance = fs::canonicalize(src.join("retired_backfill_provenance.rs"))
        .expect("canonical provenance source");
    let generic_runtime = fs::canonicalize(src.join("retired_backfill_evidence.rs"))
        .expect("canonical generic source");

    for needle in ["binance", "bybit", "bnbusdc"] {
        assert!(needle_allowed_in_production_path(
            needle,
            &provenance,
            crate_root
        ));
        assert!(!needle_allowed_in_production_path(
            needle,
            &generic_runtime,
            crate_root
        ));
    }
    for needle in ["pmxt", "polymarket", "public_archive"] {
        assert!(!needle_allowed_in_production_path(
            needle,
            &provenance,
            crate_root
        ));
    }
}

#[test]
fn reference_fixture_index_sample_allowlist_is_limited_to_provenance_terms() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = fs::canonicalize(crate_root.join("src/reference_fixture_index.rs"))
        .expect("canonical fixture index source");

    for needle in ["binance", "bybit", "pmxt", "polymarket"] {
        assert!(needle_allowed_in_production_path(needle, &path, crate_root));
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
        assert!(!needle_allowed_in_production_path(
            needle, &path, crate_root
        ));
    }
}

fn synthetic_manifest() -> String {
    r#"
[package]
name = "synthetic-production-source-guard"
version = "0.0.0"
edition = "2024"
autobins = false
autoexamples = false
autobenches = false
build = false
"#
    .to_owned()
}

fn write_synthetic_source(crate_root: &Path, relative_path: &str, source: &str) {
    let path = crate_root.join(relative_path);
    fs::create_dir_all(path.parent().expect("synthetic source parent"))
        .expect("create synthetic source parent");
    fs::write(path, source).expect("write synthetic source");
}
