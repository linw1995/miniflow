use proc_macro2::{LineColumn, TokenStream, TokenTree};
use quote::ToTokens;
use std::collections::BTreeMap;
use syn::{Item, spanned::Spanned, visit::Visit};

struct TestBody {
    start: usize,
    end: usize,
}

/// Restrict narrowing to private, directly declared tests in an isolated source scope.
/// Unknown macros and out-of-line children can hide calls to otherwise private tests.
pub fn isolated_changes(before: &str, after: &str, integration: bool) -> Option<Vec<String>> {
    let old = tests(before, integration)?;
    let new = tests(after, integration)?;
    if old.keys().ne(new.keys()) || old.is_empty() {
        return None;
    }
    let mut old_masked = before.to_owned();
    let mut new_masked = after.to_owned();
    let changed: Vec<_> = old
        .iter()
        .filter_map(|(name, body)| {
            let next = &new[name];
            (before[body.start..body.end] != after[next.start..next.end]).then(|| name.clone())
        })
        .collect();
    for (text, entries) in [(&mut old_masked, &old), (&mut new_masked, &new)] {
        let mut bodies: Vec<_> = entries.iter().collect();
        bodies.sort_by_key(|(_, body)| std::cmp::Reverse(body.start));
        for (name, body) in bodies {
            text.replace_range(body.start..body.end, &format!("{{__isolated_{name}__}}"));
        }
    }
    (old_masked == new_masked && !changed.is_empty()).then_some(changed)
}

fn offset(source: &str, position: LineColumn) -> Option<usize> {
    if position.line == 0 {
        return None;
    }
    let start: usize = source
        .split_inclusive('\n')
        .take(position.line - 1)
        .map(str::len)
        .sum();
    let result = start.checked_add(position.column)?;
    (result <= source.len() && source.is_char_boundary(result)).then_some(result)
}

fn identifier_count(tokens: TokenStream, name: &str) -> usize {
    tokens
        .into_iter()
        .map(|token| match token {
            TokenTree::Ident(ident) => usize::from(ident == name),
            TokenTree::Group(group) => identifier_count(group.stream(), name),
            _ => 0,
        })
        .sum()
}

struct ScopeCheck {
    valid: bool,
    json_imported: bool,
}
impl<'ast> Visit<'ast> for ScopeCheck {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if item.content.is_none() {
            self.valid = false;
        }
        syn::visit::visit_item_mod(self, item);
    }
    fn visit_item_macro(&mut self, _: &'ast syn::ItemMacro) {
        self.valid = false;
    }
    fn visit_expr_path(&mut self, item: &'ast syn::ExprPath) {
        // Resource/process access defeats the narrow, local-test isolation contract.
        if item.path.segments.iter().any(|s| {
            matches!(
                s.ident.to_string().as_str(),
                "fs" | "env" | "process" | "thread" | "unsafe"
            )
        }) {
            self.valid = false;
        }
        syn::visit::visit_expr_path(self, item);
    }
    fn visit_expr_unsafe(&mut self, _: &'ast syn::ExprUnsafe) {
        self.valid = false;
    }
    fn visit_item_static(&mut self, _: &'ast syn::ItemStatic) {
        self.valid = false;
    }
    fn visit_macro(&mut self, item: &'ast syn::Macro) {
        let path = item.path.to_token_stream().to_string().replace(' ', "");
        if !matches!(
            path.as_str(),
            "assert"
                | "assert_eq"
                | "assert_ne"
                | "matches"
                | "vec"
                | "format"
                | "panic"
                | "serde_json::json"
        ) && !(path == "json" && self.json_imported)
        {
            self.valid = false;
        }
        syn::visit::visit_macro(self, item);
    }
}

fn tests(source: &str, integration: bool) -> Option<BTreeMap<String, TestBody>> {
    let file = syn::parse_file(source).ok()?;
    let mut output = BTreeMap::new();
    collect(&file.items, source, "", integration, &mut output)?;
    Some(output)
}

fn collect(
    items: &[Item],
    source: &str,
    prefix: &str,
    test_scope: bool,
    output: &mut BTreeMap<String, TestBody>,
) -> Option<()> {
    if test_scope {
        let tokens: TokenStream = items.iter().map(ToTokens::to_token_stream).collect();
        let json_imported = items.iter().any(|item| matches!(item, Item::Use(u) if u.to_token_stream().to_string().replace(' ', "").contains("serde_json::json")));
        let mut check = ScopeCheck {
            valid: true,
            json_imported,
        };
        for item in items {
            check.visit_item(item);
        }
        if !check.valid {
            return None;
        }
        for item in items {
            if let Item::Fn(function) = item
                && function
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("test"))
            {
                if !matches!(function.vis, syn::Visibility::Inherited)
                    || identifier_count(tokens.clone(), &function.sig.ident.to_string()) != 1
                {
                    return None;
                }
                let span = function.block.span();
                output.insert(
                    format!("{prefix}{}", function.sig.ident),
                    TestBody {
                        start: offset(source, span.start())?,
                        end: offset(source, span.end())?,
                    },
                );
            }
        }
    }
    for item in items {
        if let Item::Mod(module) = item
            && let Some((_, children)) = &module.content
        {
            let guarded = module.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && attr
                        .parse_args::<syn::Path>()
                        .is_ok_and(|path| path.is_ident("test"))
            });
            collect(
                children,
                source,
                &format!("{prefix}{}::", module.ident),
                test_scope || guarded,
                output,
            )?;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_isolated_bodies_from_shared_and_production_changes() {
        let source = "fn production() {}\n#[cfg(test)] mod tests { #[test] fn isolated() { assert_eq!(1, 1); } }";
        assert_eq!(
            isolated_changes(source, &source.replace("1, 1", "2, 2"), false),
            Some(vec!["tests::isolated".into()])
        );
        assert!(
            isolated_changes(
                source,
                &source.replace("production() {}", "production() { panic!(); }"),
                false
            )
            .is_none()
        );
        assert!(
            isolated_changes(
                source,
                &source.replace("#[test]", "#[ignore] #[test]"),
                false
            )
            .is_none()
        );
        for extra in [
            "fn shared() { isolated(); }",
            "mod hidden;",
            "register!();",
            "#[test] fn another() { custom!(); }",
        ] {
            let modified = source.replacen("mod tests {", &format!("mod tests {{ {extra}"), 1);
            assert!(
                isolated_changes(&modified, &modified.replace("1, 1", "2, 2"), false).is_none(),
                "{extra}"
            );
        }
    }

    #[test]
    fn handles_unicode_before_a_changed_body() {
        let before = "// \u{1f600}\n#[cfg(test)] mod tests { #[test] fn example() { assert_eq!(\"\u{e9}\", \"\u{e9}\"); } }";
        let after = before.replace("assert_eq!", "assert_ne!");
        assert_eq!(
            isolated_changes(before, &after, false),
            Some(vec!["tests::example".into()])
        );
    }
}
