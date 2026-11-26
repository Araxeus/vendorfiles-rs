//! Dependency aliases: the checks a config passes on load, and name lookup through them.

use std::collections::HashMap;

use indexmap::IndexMap;

use super::Workspace;
use crate::error::{Result, VendorError};
use crate::model::RawDependency;

/// Refuses a malformed `alias`, an `alias` in the `default` block, and an alias that names two
/// things.
///
/// The shape is read from `canonical`, because the lenient reader turns a wrong type into no
/// alias at all.
pub(super) fn check(
    canonical: &serde_json::Value,
    dependencies: &IndexMap<String, RawDependency>,
) -> Result<()> {
    for key in ["defaultVendorOptions", "default"] {
        if canonical
            .get(key)
            .and_then(|block| block.get("alias"))
            .is_some_and(|alias| !alias.is_null())
        {
            return Err(VendorError::AliasInDefaults {
                key: key.to_owned(),
            });
        }
    }

    let blocks = canonical
        .get("vendorDependencies")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten();
    for (name, block) in blocks {
        match block.get("alias") {
            None | Some(serde_json::Value::Null) => {}
            Some(alias) if well_formed(alias) => {}
            Some(_) => return Err(VendorError::InvalidAliasKey { name: name.clone() }),
        }
    }

    let mut owners: HashMap<&str, &str> = dependencies
        .keys()
        .map(|name| (name.as_str(), name.as_str()))
        .collect();
    for (name, dependency) in dependencies {
        for alias in dependency.alias.iter().flat_map(|aliases| &aliases.0) {
            if let Some(owner) = owners.get(alias.as_str()) {
                return Err(VendorError::AliasTaken {
                    name: name.clone(),
                    alias: alias.clone(),
                    owner: (*owner).to_owned(),
                });
            }
            owners.insert(alias, name);
        }
    }
    Ok(())
}

/// Whether `alias` is a non-empty name or a non-empty list of them.
fn well_formed(alias: &serde_json::Value) -> bool {
    let name = |value: &serde_json::Value| value.as_str().is_some_and(|s| !s.is_empty());
    match alias {
        serde_json::Value::Array(names) => !names.is_empty() && names.iter().all(name),
        other => name(other),
    }
}

impl Workspace {
    /// The config key `typed` names, as the key itself or one of its aliases.
    #[must_use]
    pub fn dependency_name(&self, typed: &str) -> Option<&str> {
        dependency_name(&self.dependencies, typed)
    }

    /// Whether `name` already names a dependency, as a key or an alias.
    #[must_use]
    pub fn is_taken(&self, name: &str) -> bool {
        self.dependency_name(name).is_some()
    }
}

fn dependency_name<'a>(
    dependencies: &'a IndexMap<String, RawDependency>,
    typed: &str,
) -> Option<&'a str> {
    if let Some((key, _)) = dependencies.get_key_value(typed) {
        return Some(key);
    }
    dependencies
        .iter()
        .find(|(_, dependency)| {
            dependency
                .alias
                .as_ref()
                .is_some_and(|aliases| aliases.0.iter().any(|alias| alias == typed))
        })
        .map(|(key, _)| key.as_str())
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use crate::model::RawDependency;

    fn dependencies(json: &str) -> IndexMap<String, RawDependency> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_name_is_found_by_its_key_or_an_alias() {
        let dependencies = dependencies(r#"{"ripgrep": {"alias": ["rg", "grep2"]}, "fd": {}}"#);
        assert_eq!(
            super::dependency_name(&dependencies, "ripgrep"),
            Some("ripgrep")
        );
        assert_eq!(
            super::dependency_name(&dependencies, "grep2"),
            Some("ripgrep")
        );
        assert_eq!(super::dependency_name(&dependencies, "fd"), Some("fd"));
        assert_eq!(super::dependency_name(&dependencies, "bat"), None);
    }
}
