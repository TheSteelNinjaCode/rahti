use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use fluent_syntax::ast;
use fluent_syntax::parser;
use unic_langid::LanguageIdentifier;

use crate::{I18n, I18nResource};

pub fn parse(value: &serde_json::Value) -> Option<I18n> {
    let raw = value.get("i18n")?;
    let object = raw
        .as_object()
        .unwrap_or_else(|| panic!("rahti: i18n must be an object"));
    let default = canonical(
        object
            .get("default")
            .and_then(|value| value.as_str())
            .unwrap_or("en"),
        "i18n.default",
    );
    let locales = match object.get("locales") {
        Some(value) => value
            .as_array()
            .unwrap_or_else(|| panic!("rahti: i18n.locales must be an array"))
            .iter()
            .map(|value| {
                canonical(
                    value.as_str().unwrap_or_else(|| {
                        panic!("rahti: every i18n.locales entry must be a string")
                    }),
                    "i18n.locales",
                )
            })
            .collect::<Vec<_>>(),
        None => vec![default.clone()],
    };
    if locales.is_empty() {
        panic!("rahti: i18n.locales must contain at least one locale");
    }
    let unique = locales.iter().collect::<HashSet<_>>();
    if unique.len() != locales.len() {
        panic!("rahti: i18n.locales contains a duplicate locale");
    }
    if !locales.contains(&default) {
        panic!("rahti: i18n.default `{default}` is not listed in i18n.locales");
    }

    let mut fallbacks = Vec::new();
    if let Some(raw) = object.get("fallbacks") {
        let raw = raw
            .as_object()
            .unwrap_or_else(|| panic!("rahti: i18n.fallbacks must be an object"));
        for (locale, chain) in raw {
            let locale = canonical(locale, "i18n.fallbacks key");
            if !locales.contains(&locale) {
                panic!("rahti: i18n fallback source `{locale}` is not a configured locale");
            }
            let chain = chain
                .as_array()
                .unwrap_or_else(|| panic!("rahti: i18n.fallbacks.{locale} must be an array"))
                .iter()
                .map(|value| {
                    let fallback = canonical(
                        value.as_str().unwrap_or_else(|| {
                            panic!("rahti: every fallback for `{locale}` must be a string")
                        }),
                        "i18n fallback",
                    );
                    if !locales.contains(&fallback) {
                        panic!(
                            "rahti: fallback `{fallback}` for `{locale}` is not in i18n.locales"
                        );
                    }
                    fallback
                })
                .collect::<Vec<_>>();
            fallbacks.push((locale, chain));
        }
    }
    fallbacks.sort_by(|a, b| a.0.cmp(&b.0));
    reject_cycles(&fallbacks);

    let dir = object
        .get("dir")
        .and_then(|value| value.as_str())
        .unwrap_or("lang")
        .to_string();
    Some(I18n {
        default,
        locales,
        fallbacks,
        dir,
        resources: Vec::new(),
    })
}

fn canonical(value: &str, field: &str) -> String {
    value
        .parse::<LanguageIdentifier>()
        .unwrap_or_else(|_| panic!("rahti: {field} `{value}` is not a valid locale identifier"))
        .to_string()
}

fn reject_cycles(fallbacks: &[(String, Vec<String>)]) {
    for (locale, chain) in fallbacks {
        let mut seen = HashSet::new();
        for fallback in chain {
            if fallback == locale || !seen.insert(fallback) {
                panic!(
                    "rahti: i18n fallback chain for `{locale}` repeats `{fallback}` or falls back to itself"
                );
            }
        }
    }
}

pub fn scan(config: &mut I18n) {
    let root = Path::new(&config.dir);
    if !root.exists() {
        panic!(
            "rahti: i18n is configured, but its catalog directory `{}` does not exist",
            config.dir
        );
    }
    let mut resources = Vec::new();
    for locale in &config.locales {
        let directory = root.join(locale);
        if !directory.is_dir() {
            panic!(
                "rahti: locale `{locale}` has no catalog directory `{}`",
                directory.display()
            );
        }
        collect(&directory, locale, &mut resources);
    }
    resources.sort_by(|a, b| a.path.cmp(&b.path));

    let mut identifiers: HashMap<(String, String), String> = HashMap::new();
    for resource in &resources {
        let parsed = parser::parse(resource.source.as_str()).unwrap_or_else(|(_, errors)| {
            panic!("rahti: {} is not valid Fluent: {:?}", resource.path, errors)
        });
        for entry in parsed.body {
            if let ast::Entry::Message(message) = entry {
                let key = (resource.locale.clone(), message.id.name.to_string());
                if let Some(first) = identifiers.insert(key.clone(), resource.path.clone()) {
                    panic!(
                        "rahti: Fluent message `{}` for `{}` is declared in both {} and {}",
                        key.1, key.0, first, resource.path
                    );
                }
            }
        }
    }
    config.resources = resources;
}

fn collect(directory: &Path, locale: &str, output: &mut Vec<I18nResource>) {
    let mut paths = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("rahti: cannot read {}: {error}", directory.display()))
        .map(|entry| {
            entry
                .expect("rahti: catalog directory entry disappeared")
                .path()
        })
        .collect::<Vec<PathBuf>>();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(&path, locale, output);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("ftl") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("rahti: cannot read {}: {error}", path.display()));
            output.push(I18nResource {
                locale: locale.to_string(),
                path: path.to_string_lossy().replace('\\', "/"),
                source,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Tree;

    fn configured(tree: &Tree) -> I18n {
        I18n {
            default: "en".into(),
            locales: vec!["en".into(), "es".into()],
            fallbacks: vec![("es".into(), vec!["en".into()])],
            dir: tree.path().to_string_lossy().into_owned(),
            resources: Vec::new(),
        }
    }

    #[test]
    fn catalogs_are_validated_sorted_and_loaded() {
        let tree = Tree::new("i18n-catalogs");
        tree.file("en/z.ftl", "z = Last")
            .file("en/nested/a.ftl", "a = First")
            .file("es/app.ftl", "a = Primero");
        let mut config = configured(&tree);
        scan(&mut config);
        assert_eq!(config.resources.len(), 3);
        assert!(config.resources[0].path.ends_with("en/nested/a.ftl"));
        assert_eq!(config.resources[2].locale, "es");
    }

    #[test]
    #[should_panic(expected = "is declared in both")]
    fn duplicate_messages_in_one_locale_are_refused() {
        let tree = Tree::new("i18n-duplicate");
        tree.file("en/a.ftl", "same = First")
            .file("en/b.ftl", "same = Second")
            .file("es/app.ftl", "same = Otro");
        scan(&mut configured(&tree));
    }

    #[test]
    #[should_panic(expected = "is not valid Fluent")]
    fn malformed_fluent_is_refused() {
        let tree = Tree::new("i18n-invalid");
        tree.file("en/app.ftl", "message = {")
            .file("es/app.ftl", "message = Bien");
        scan(&mut configured(&tree));
    }
}
