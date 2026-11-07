//! Syntax helpers for bounded Rust source-policy gates.
//!
//! ```
//! use enzui_test_support::rust_syntax::{imports, syn};
//!
//! let item: syn::ItemUse = syn::parse_str("use iced::Task as Job;").unwrap();
//! let imported = imports(&item);
//! assert_eq!(imported[0].local.as_deref(), Some("Job"));
//! assert_eq!(imported[0].path, "iced::Task");
//! ```
//!
//! Parse original source with [`syn::parse_file`], then exclude test-only nodes;
//! a literal-masked text projection is not necessarily valid Rust. These helpers
//! do not expand macros, resolve types, or supply lexical scopes. Callers own
//! scope/shadowing rules and must not treat unresolved imports as reviewed calls.

pub use syn;

use std::collections::BTreeMap;
use syn::{
    Expr, Pat, UseTree,
    ext::IdentExt,
    visit::{self, Visit},
};

/// One explicit import or an unexpanded glob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Local name, or `None` for a glob whose exports remain unresolved.
    pub local: Option<String>,
    /// Imported path, retaining an absolute prefix but excluding a trailing glob marker.
    pub path: String,
}

/// Extract imports without discarding duplicate bindings or expanding globs.
#[must_use]
pub fn imports(item: &syn::ItemUse) -> Vec<Import> {
    fn walk(tree: &UseTree, prefix: &str, out: &mut Vec<Import>) {
        match tree {
            UseTree::Path(path) => walk(
                &path.tree,
                &format!("{prefix}{}::", path.ident.unraw()),
                out,
            ),
            UseTree::Name(name) if name.ident == "self" => {
                let path = prefix.trim_end_matches("::");
                out.push(Import {
                    local: Some(path.rsplit("::").next().unwrap_or("self").to_owned()),
                    path: path.to_owned(),
                });
            }
            UseTree::Name(name) => out.push(Import {
                local: Some(name.ident.unraw().to_string()),
                path: format!("{prefix}{}", name.ident.unraw()),
            }),
            UseTree::Rename(rename) => out.push(Import {
                local: Some(rename.rename.unraw().to_string()),
                path: if rename.ident == "self" {
                    prefix.trim_end_matches("::").to_owned()
                } else {
                    format!("{prefix}{}", rename.ident.unraw())
                },
            }),
            UseTree::Group(group) => {
                for item in &group.items {
                    walk(item, prefix, out);
                }
            }
            UseTree::Glob(_) => out.push(Import {
                local: None,
                path: prefix.trim_end_matches("::").to_owned(),
            }),
        }
    }
    let mut result = Vec::new();
    walk(
        &item.tree,
        if item.leading_colon.is_some() {
            "::"
        } else {
            ""
        },
        &mut result,
    );
    result
}

/// Normalize path segments, retaining an absolute prefix but excluding generics and raw prefixes.
#[must_use]
pub fn path_name(path: &syn::Path) -> String {
    let name = path
        .segments
        .iter()
        .map(|segment| segment.ident.unraw().to_string())
        .collect::<Vec<_>>()
        .join("::");
    if path.leading_colon.is_some() {
        format!("::{name}")
    } else {
        name
    }
}

/// Match complete trailing path segments, never an identifier suffix.
#[must_use]
pub fn has_path_tail(name: &str, tail: &str) -> bool {
    !tail.is_empty()
        && (name == tail
            || name
                .strip_suffix(tail)
                .is_some_and(|prefix| prefix.ends_with("::")))
}

/// Expand an explicitly supplied alias map; return `None` for cyclic expansion.
///
/// Absolute paths and alias targets prefixed with `::` bypass local aliases;
/// the result omits that leading separator. Identity aliases are fixed points.
/// Unknown heads remain unchanged, so expansion alone never establishes that
/// a path denotes a trusted Rust item.
#[must_use]
pub fn resolve_path(path: &syn::Path, aliases: &BTreeMap<String, String>) -> Option<String> {
    let mut name = path_name(path);
    if path.leading_colon.is_some() {
        return Some(name.trim_start_matches("::").to_owned());
    }
    for _ in 0..=aliases.len() {
        let (head, tail) = name.split_once("::").unwrap_or((&name, ""));
        let Some(prefix) = aliases.get(head) else {
            return Some(name);
        };
        let next = if tail.is_empty() {
            prefix.clone()
        } else {
            format!("{prefix}::{tail}")
        };
        if let Some(absolute) = next.strip_prefix("::") {
            return Some(absolute.to_owned());
        }
        if next == name {
            return Some(name);
        }
        name = next;
    }
    None
}

/// Identify attributes that require `cfg(test)`, preserving other feature branches.
#[must_use]
pub fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| match &attr.meta {
        syn::Meta::List(list) if list.path.is_ident("cfg") => {
            crate::hand_roll_scan::attribute_gates_on_cfg_test(&format!("#[cfg({})]", list.tokens))
        }
        _ => false,
    })
}

/// Remove only parentheses and token-stream group wrappers.
#[must_use]
pub fn unparen(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unparen(&paren.expr),
        Expr::Group(group) => unparen(&group.expr),
        _ => expr,
    }
}

/// Return the final expression only when it has no semicolon.
#[must_use]
pub fn block_tail(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last()? {
        syn::Stmt::Expr(expr, None) => Some(expr),
        _ => None,
    }
}

/// Test a named field directly on `self`, allowing parenthesized receivers.
#[must_use]
pub fn self_field(expr: &Expr, field: &str) -> bool {
    matches!(unparen(expr), Expr::Field(value)
        if matches!(&value.member, syn::Member::Named(name) if name.unraw() == field)
        && matches!(unparen(&value.base), Expr::Path(path) if path.path.is_ident("self")))
}

/// Collect binding names from a pattern, retaining nested and alternative bindings.
#[must_use]
pub fn binding_names(pattern: &Pat) -> Vec<String> {
    struct Names(Vec<String>);
    impl<'ast> Visit<'ast> for Names {
        fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
            self.0.push(pattern.ident.unraw().to_string());
            visit::visit_pat_ident(self, pattern);
        }
    }
    let mut names = Names(Vec::new());
    names.visit_pat(pattern);
    names.0
}

/// Collect `let`-expression bindings for conservative condition-scope shadowing.
///
/// This is a syntactic inventory, not a definite-assignment or control-flow analysis.
#[must_use]
pub fn condition_binding_names(expression: &Expr) -> Vec<String> {
    struct Bindings(Vec<String>);
    impl<'ast> Visit<'ast> for Bindings {
        fn visit_expr_let(&mut self, expression: &'ast syn::ExprLet) {
            self.0.extend(binding_names(&expression.pat));
            visit::visit_expr_let(self, expression);
        }
    }
    let mut bindings = Bindings(Vec::new());
    bindings.visit_expr(expression);
    bindings.0
}
