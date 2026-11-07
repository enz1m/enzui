use super::*;
use enzui_test_support::rust_syntax::{path_name, syn, test_only};
use proc_macro2::{TokenStream, TokenTree};
use std::path::Path;
use syn::parse::{Parse, ParseStream};
use syn::visit::{self, Visit};

struct MessageCall {
    key: syn::Expr,
    arguments: Vec<String>,
}

impl Parse for MessageCall {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let key = input.parse()?;
        let mut arguments = Vec::new();
        while !input.is_empty() {
            input.parse::<syn::Token![,]>()?;
            if input.is_empty() {
                break;
            }
            arguments.push(input.parse::<syn::LitStr>()?.value());
            input.parse::<syn::Token![=>]>()?;
            input.parse::<syn::Expr>()?;
        }
        Ok(Self { key, arguments })
    }
}

#[derive(Default)]
struct MessageReferences {
    calls: Vec<(String, Vec<String>)>,
}

impl MessageReferences {
    fn record(&mut self, call: MessageCall) {
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(key),
            ..
        }) = call.key
        {
            let key = key.value();
            if key.starts_with("enzui-") {
                self.calls.push((key, call.arguments));
            }
        }
    }

    fn tokens(&mut self, tokens: TokenStream) {
        let tokens: Vec<_> = tokens.into_iter().collect();
        for (index, token) in tokens.iter().enumerate() {
            if let TokenTree::Group(group) = token {
                self.tokens(group.stream());
            }
            let TokenTree::Ident(name) = token else {
                continue;
            };
            if name == "localized"
                && matches!(tokens.get(index + 1), Some(TokenTree::Punct(punctuation)) if punctuation.as_char() == '!')
                && let Some(TokenTree::Group(group)) = tokens.get(index + 2)
            {
                self.record(syn::parse2(group.stream()).expect("localized! arguments must parse"));
            }
            if name == "localization"
                && matches!(tokens.get(index + 1), Some(TokenTree::Punct(punctuation)) if punctuation.as_char() == ':')
                && matches!(tokens.get(index + 2), Some(TokenTree::Punct(punctuation)) if punctuation.as_char() == ':')
                && matches!(tokens.get(index + 3), Some(TokenTree::Ident(function)) if function == "text")
                && let Some(TokenTree::Group(group)) = tokens.get(index + 4)
            {
                self.record(syn::parse2(group.stream()).expect("text arguments must parse"));
            }
        }
    }
}

impl<'ast> Visit<'ast> for MessageReferences {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if !test_only(&function.attrs) {
            visit::visit_item_fn(self, function);
        }
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !test_only(&module.attrs) {
            visit::visit_item_mod(self, module);
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(function) = call.func.as_ref()
            && matches!(
                path_name(&function.path).as_str(),
                "crate::localization::text" | "enzui_core::localization::text"
            )
            && let Some(key) = call.args.first()
        {
            self.record(MessageCall {
                key: key.clone(),
                arguments: Vec::new(),
            });
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if invocation.path.is_ident("macro_rules") {
            return;
        }
        if invocation
            .path
            .segments
            .last()
            .is_some_and(|name| name.ident == "localized")
        {
            self.record(
                syn::parse2(invocation.tokens.clone()).expect("localized! arguments must parse"),
            );
        }
        self.tokens(invocation.tokens.clone());
    }
}

fn source_files(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read framework source directory") {
        let path = entry.expect("read source entry").path();
        let name = path
            .file_name()
            .expect("source entry has a file name")
            .to_string_lossy();
        if name == "tests"
            || name.ends_with("_tests.rs")
            || name == "tests.rs"
            || name == "catalog_contract.rs"
        {
            continue;
        }
        if path.is_dir() {
            source_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn localization_framework_message_references_match_the_english_catalog_and_argument_schema() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    source_files(&crates.join("enzui-core/src"), &mut files);
    source_files(&crates.join("enzui-widgets/src"), &mut files);
    assert!(files.len() > 400, "source walk must cover the framework");
    let localizer = Localizer::new(
        Catalog::default(),
        vec![Locale::default()],
        Locale::default(),
    );
    let mut references = MessageReferences::default();
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        references.visit_file(
            &syn::parse_file(&source).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
        );
    }
    assert!(
        references.calls.len() > 150,
        "message walk must find framework call sites"
    );
    let mut failures = Vec::new();
    for (key, names) in references.calls {
        let mut arguments = Arguments::new();
        for name in &names {
            arguments.set(name.as_str(), 2);
        }
        if let Err(error) = localizer.try_text(&key, Some(&arguments)) {
            failures.push(format!("{key} with {names:?}: {error}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
