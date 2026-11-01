use crate::model::{Change, Comparison, TestCase};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::Item;

/// Added or modified test functions found in one changed file.
pub struct TestChanges {
    /// Exact identities of the changed test functions.
    pub cases: Vec<TestCase>,
    /// True when every non-test item is unchanged and no test was removed or
    /// renamed, so the cases fully describe the file's change.
    pub exclusive: bool,
}

/// Finds test functions that were added or modified even when the same file
/// also changed production code. These cases must still execute: a coverage
/// graph cannot select new test bodies because it indexes only base-side lines.
///
/// `Ok(None)` means no test function changed. `Err(())` means changed tests
/// exist but their identities cannot be proven, so callers must widen scope.
pub fn changed<'a>(
    comparison: &Comparison,
    change: &Change,
    package: &str,
    targets: impl Iterator<Item = (&'a str, &'a [String], &'a Path)>,
) -> Result<Option<TestChanges>, ()> {
    if change.deleted || !change.path.ends_with(".rs") {
        return Ok(None);
    }
    let path = comparison.head_root.join(&change.path);
    let base_path = comparison.base_root.join(&change.path);
    let before = match fs::read_to_string(base_path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err(()),
    };
    let mut before = syn::parse_file(&before).map_err(|_| ())?;
    let mut after = parse(&path).ok_or(())?;
    let mut old_tests = BTreeMap::new();
    let mut new_tests = BTreeMap::new();
    extract(&mut before.items, "", &mut old_tests).ok_or(())?;
    extract(&mut after.items, "", &mut new_tests).ok_or(())?;
    let exclusive = before.to_token_stream().to_string() == after.to_token_stream().to_string()
        // Deletions and renames cannot be exclusive: no surviving identity
        // validates the removed case against the runnable inventory.
        && !old_tests.keys().any(|name| !new_tests.contains_key(name));
    let changed: BTreeSet<_> = new_tests
        .iter()
        .filter(|(name, tokens)| old_tests.get(*name) != Some(*tokens))
        .map(|(name, _)| name.clone())
        .collect();
    if changed.is_empty() {
        return Ok(None);
    }
    let names: BTreeSet<_> = changed
        .iter()
        .map(|name| name.rsplit("::").next().unwrap().to_owned())
        .collect();
    let mut remaining = after.to_token_stream();
    for (name, tokens) in &new_tests {
        if !changed.contains(name) {
            remaining.extend(tokens.parse::<TokenStream>().map_err(|_| ())?);
        }
    }
    if references(remaining, &names) {
        return Err(());
    }

    let mut selected = BTreeSet::new();
    for (name, kind, source) in targets {
        let binary = if kind
            .iter()
            .any(|kind| kind == "lib" || kind == "proc-macro")
        {
            package.to_owned()
        } else if kind.iter().any(|kind| kind == "test") {
            format!("{package}::{name}")
        } else if kind.iter().any(|kind| kind == "bin") {
            format!("{package}::bin/{name}")
        } else {
            return Err(());
        };
        let mut files = BTreeMap::new();
        modules(source, "", source.parent().ok_or(())?, &mut files).ok_or(())?;
        let Some((prefix, _)) = files.get(&path) else {
            continue;
        };
        // A test can itself be called as a helper. Such callers must not be
        // omitted just because the callee carries a test attribute.
        if files
            .iter()
            .any(|(file, (_, tokens))| file != &path && references(tokens.clone(), &names))
        {
            return Err(());
        }
        for name in &changed {
            selected.insert(TestCase {
                package: package.into(),
                binary: binary.clone(),
                name: qualify(prefix, name),
            });
        }
    }
    if selected.is_empty() {
        return Err(());
    }
    Ok(Some(TestChanges {
        cases: selected.into_iter().collect(),
        exclusive,
    }))
}

fn parse(path: &Path) -> Option<syn::File> {
    syn::parse_file(&fs::read_to_string(path).ok()?).ok()
}

fn qualify(prefix: &str, name: &str) -> String {
    let name = name.trim_start_matches("r#");
    if prefix.is_empty() {
        name.into()
    } else {
        format!("{prefix}::{name}")
    }
}

fn extract(
    items: &mut Vec<Item>,
    prefix: &str,
    tests: &mut BTreeMap<String, String>,
) -> Option<()> {
    let mut retained = Vec::new();
    for mut item in items.drain(..) {
        match &mut item {
            Item::Fn(function)
                if function.attrs.iter().any(|attr| {
                    let path = attr.path().to_token_stream().to_string();
                    matches!(
                        path.as_str(),
                        "test" | "tokio :: test" | "async_std :: test"
                    )
                }) && function.attrs.iter().all(|attr| {
                    let path = attr.path().to_token_stream().to_string();
                    matches!(
                        path.as_str(),
                        "test" | "tokio :: test" | "async_std :: test" | "should_panic" | "doc"
                    )
                }) =>
            {
                let name = qualify(prefix, &function.sig.ident.to_string());
                if tests
                    .insert(name, function.to_token_stream().to_string())
                    .is_some()
                {
                    return None;
                }
                continue;
            }
            Item::Mod(module) => {
                if let Some((_, items)) = &mut module.content {
                    extract(items, &qualify(prefix, &module.ident.to_string()), tests)?;
                }
            }
            _ => {}
        }
        retained.push(item);
    }
    *items = retained;
    Some(())
}

fn references(tokens: TokenStream, names: &BTreeSet<String>) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => names.contains(ident.to_string().trim_start_matches("r#")),
        TokenTree::Group(group) => references(group.stream(), names),
        _ => false,
    })
}

fn modules(
    path: &Path,
    prefix: &str,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, (String, TokenStream)>,
) -> Option<()> {
    // Bound recursive or ambiguous module layouts; they retain package scope.
    if files.len() >= 512 || files.contains_key(path) {
        return None;
    }
    let syntax = parse(path)?;
    files.insert(path.into(), (prefix.into(), syntax.to_token_stream()));
    child_modules(&syntax.items, prefix, directory, files)
}

fn child_modules(
    items: &[Item],
    prefix: &str,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, (String, TokenStream)>,
) -> Option<()> {
    for item in items {
        if let Item::Macro(item) = item
            && item.mac.path.is_ident("include")
        {
            return None;
        }
        let Item::Mod(module) = item else {
            continue;
        };
        if module
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("path") || attr.path().is_ident("cfg_attr"))
        {
            return None;
        }
        let name = module.ident.to_string();
        let prefix = qualify(prefix, &name);
        let nested = directory.join(&name);
        if let Some((_, items)) = &module.content {
            child_modules(items, &prefix, &nested, files)?;
        } else {
            let candidates = [directory.join(format!("{name}.rs")), nested.join("mod.rs")];
            let mut candidates = candidates.iter().filter(|path| path.is_file());
            let source = candidates.next()?;
            if candidates.next().is_some() {
                return None;
            }
            modules(source, &prefix, &nested, files)?;
        }
    }
    Some(())
}
