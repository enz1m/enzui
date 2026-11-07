//! Diagnostic inventory of styling candidates; findings require semantic review.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a command-line tool reports by panicking and printing, and none of it is public API"
)]
use enzui_test_support::hand_roll_scan::walk_rs_files_follow_root;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};

#[derive(Default)]
struct Inventory {
    file: String,
    scope: Vec<String>,
    context: Vec<String>,
    counts: BTreeMap<&'static str, usize>,
    rows: Vec<String>,
}

impl Inventory {
    fn record(&mut self, kind: &'static str, value: String) {
        *self.counts.entry(kind).or_default() += 1;
        self.rows.push(format!(
            "{}\t{}\t{}\t{}\t{}",
            self.file,
            self.scope.join("::"),
            self.context.join("/"),
            kind,
            value.replace(['\n', '\t'], " ")
        ));
    }
}

fn is_test(attrs: &[syn::Attribute]) -> bool {
    enzui_test_support::rust_syntax::test_only(attrs)
        || attrs.iter().any(|a| a.path().is_ident("test"))
}

impl<'ast> Visit<'ast> for Inventory {
    fn visit_file(&mut self, node: &'ast syn::File) {
        if !is_test(&node.attrs) {
            visit::visit_file(self, node);
        }
    }
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !is_test(&node.attrs) {
            self.scope.push(node.ident.to_string());
            visit::visit_item_mod(self, node);
            self.scope.pop();
        }
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if !is_test(&node.attrs) {
            self.scope.push(node.sig.ident.to_string());
            visit::visit_item_fn(self, node);
            self.scope.pop();
        }
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.scope.push(node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.scope.pop();
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if !is_test(&node.attrs) {
            let name = match node.self_ty.as_ref() {
                syn::Type::Path(p) => p.path.segments.last().unwrap().ident.to_string(),
                _ => "impl".into(),
            };
            self.scope.push(name);
            visit::visit_item_impl(self, node);
            self.scope.pop();
        }
    }
    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.scope.push(node.ident.to_string());
        visit::visit_item_const(self, node);
        self.scope.pop();
    }
    fn visit_field_value(&mut self, node: &'ast syn::FieldValue) {
        let name = match &node.member {
            syn::Member::Named(n) => n.to_string(),
            syn::Member::Unnamed(n) => n.index.to_string(),
        };
        self.context.push(name);
        visit::visit_field_value(self, node);
        self.context.pop();
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.visit_expr(&node.receiver);
        if node.method == "px" {
            self.record("default-resolution", ".px()".into());
        }
        self.context.push(format!(".{}()", node.method));
        for arg in &node.args {
            self.visit_expr(arg);
        }
        self.context.pop();
    }
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = node.func.as_ref() {
            let name = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            if name.ends_with("Scale::default") || name.ends_with("Tokens::default") {
                self.record("default-scale", name.clone());
            }
            if name.ends_with("::px") {
                self.record("default-resolution", name.clone());
            }
            self.context.push(format!("{name}()"));
            for arg in &node.args {
                self.visit_expr(arg);
            }
            self.context.pop();
        } else {
            visit::visit_expr_call(self, node);
        }
    }
    fn visit_expr_lit(&mut self, node: &'ast syn::ExprLit) {
        match &node.lit {
            syn::Lit::Float(v) => self.record("number", v.to_string()),
            syn::Lit::Int(v) => self.record("number", v.to_string()),
            _ => (),
        }
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let name = node.path.segments.last().unwrap().ident.to_string();
        self.record("opaque-macro", format!("{name}!({})", node.tokens));
    }
}

fn main() {
    let root = std::env::args_os().nth(1).map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_owned()
        },
        PathBuf::from,
    );
    let mut audit = Inventory::default();
    let mut files = 0;
    let mut crates = std::fs::read_dir(root.join("crates"))
        .expect("framework crates")
        .map(|e| e.expect("crate entry").path())
        .filter(|p| {
            p.join("src").is_dir() && p.file_name().is_some_and(|n| n != "enzui-test-support")
        })
        .collect::<Vec<_>>();
    crates.sort();
    for crate_dir in crates {
        for relative in walk_rs_files_follow_root(&crate_dir.join("src")) {
            let file = crate_dir.join("src").join(relative);
            if file.components().any(|c| c.as_os_str() == "tests")
                || file.file_name().is_some_and(|n| n == "tests.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("read source");
            let ast =
                syn::parse_file(&source).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            files += 1;
            audit.file = file
                .strip_prefix(&root)
                .expect("framework path")
                .display()
                .to_string();
            audit.visit_file(&ast);
        }
    }
    assert!(files >= 400, "incomplete source coverage: {files}");
    assert!(
        audit.counts.get("number").copied().unwrap_or(0) >= 1000,
        "empty literal inventory"
    );
    eprintln!(
        "Scanned {files} source files outside test directories; candidates {:?}. Macros require manual review; this is not a styling compliance verdict.",
        audit.counts
    );
    println!("file\tscope\tcontext\tkind\tvalue");
    for row in audit.rows {
        println!("{row}");
    }
}
