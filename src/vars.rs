use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::bail;
use indexmap::IndexMap;

#[derive(Debug, Clone, Default)]
pub struct Scope {
    defines: Arc<IndexMap<String, String>>,
    layers: Arc<Vec<Arc<IndexMap<String, String>>>>,
}

impl Scope {
    #[must_use]
    pub fn new(defines: IndexMap<String, String>) -> Self {
        Self {
            defines: Arc::new(defines),
            layers: Arc::new(Vec::new()),
        }
    }

    #[must_use]
    pub(crate) fn set(&self, vars: Arc<IndexMap<String, String>>) -> Self {
        let mut layers = self.layers.as_ref().clone();
        layers.push(vars);
        Self {
            defines: Arc::clone(&self.defines),
            layers: Arc::new(layers),
        }
    }

    pub(crate) fn insert(&mut self, key: String, val: String) {
        let layers = Arc::make_mut(&mut self.layers);
        if layers.is_empty() {
            layers.push(Arc::new(IndexMap::new()));
        }

        Arc::make_mut(layers.last_mut().unwrap()).insert(key, val);
    }

    #[must_use]
    pub fn flatten(&self) -> IndexMap<String, String> {
        let mut result = IndexMap::new();
        for layer in self.layers.iter() {
            result.extend(layer.iter().map(|(key, val)| (key.clone(), val.clone())));
        }

        result.extend(self.defines.iter().map(|(key, val)| (key.clone(), val.clone())));
        result
    }
}

impl Vars for Scope {
    fn get(&self, key: &str) -> Option<&str> {
        self.defines.get(key).map(String::as_str).or_else(|| {
            self.layers
                .iter()
                .rev()
                .find_map(|layer| layer.get(key).map(String::as_str))
        })
    }
}

impl Vars for &Scope {
    fn get(&self, key: &str) -> Option<&str> {
        (*self).get(key)
    }
}

pub trait Vars {
    fn get(&self, key: &str) -> Option<&str>;
}

impl Vars for BTreeMap<String, String> {
    fn get(&self, key: &str) -> Option<&str> {
        BTreeMap::get(self, key).map(String::as_str)
    }
}

impl Vars for IndexMap<String, String> {
    fn get(&self, key: &str) -> Option<&str> {
        IndexMap::get(self, key).map(String::as_str)
    }
}

impl Vars for &IndexMap<String, String> {
    fn get(&self, key: &str) -> Option<&str> {
        IndexMap::get(*self, key).map(String::as_str)
    }
}

impl Vars for &BTreeMap<String, String> {
    fn get(&self, key: &str) -> Option<&str> {
        BTreeMap::get(*self, key).map(String::as_str)
    }
}

impl Vars for Cow<'_, BTreeMap<String, String>> {
    fn get(&self, key: &str) -> Option<&str> {
        BTreeMap::get(self.as_ref(), key).map(String::as_str)
    }
}

impl Vars for &Cow<'_, BTreeMap<String, String>> {
    fn get(&self, key: &str) -> Option<&str> {
        BTreeMap::get(self.as_ref(), key).map(String::as_str)
    }
}

impl Vars for &[&BTreeMap<String, String>] {
    fn get(&self, key: &str) -> Option<&str> {
        self.iter().find_map(|map| map.get(key))
    }
}

#[must_use]
pub fn is_ident(src: &str) -> bool {
    let mut bytes = src.bytes();
    bytes
        .next()
        .is_some_and(|byte| matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[allow(clippy::needless_pass_by_value)]
pub fn expand<V: Vars>(input: &str, vars: V) -> anyhow::Result<String> {
    if !input.contains("${{") {
        return Ok(input.to_owned());
    }

    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(st) = rest.find("${{") {
        result.push_str(&rest[..st]);
        let after = &rest[st + 3..];

        let Some(ed) = after.find("}}") else {
            bail!("unclosed placeholder in '{input}'");
        };

        let key = after[..ed].trim();
        if key.is_empty() {
            bail!("empty variable name in placeholder");
        }
        if !is_ident(key) {
            bail!("invalid variable name '{key}' in placeholder");
        }

        let val = vars
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("variable '{key}' not found"))?;

        result.push_str(val);
        rest = &after[ed + 2..];
    }

    result.push_str(rest);
    if result.contains("${{") {
        bail!("unexpanded variable remains in '{result}'");
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_values_and_rejects_unexpanded_templates() -> anyhow::Result<()> {
        let vars = IndexMap::from([
            ("NAME".to_owned(), "astra".to_owned()),
            ("VERSION".to_owned(), "1.0".to_owned()),
        ]);
        let scope = Scope::new(vars.clone());

        assert_eq!(scope.get("NAME"), Some("astra"));
        assert_eq!(scope.flatten(), vars);
        assert_eq!(expand("${{ NAME }}-${{ VERSION }}", &scope)?, "astra-1.0");
        assert_eq!(expand("plain", &scope)?, "plain");
        let err = expand("${{ MISSING }}", &scope).unwrap_err();
        assert!(err.to_string().contains("variable 'MISSING' not found"), "{err}");
        let err = expand("${{}}", &scope).unwrap_err();
        assert!(err.to_string().contains("empty variable name in placeholder"), "{err}");
        let err = expand("${{ 123 }}", &scope).unwrap_err();
        assert!(
            err.to_string().contains("invalid variable name '123' in placeholder"),
            "{err}"
        );
        let err = expand("${{ NAME", &scope).unwrap_err();
        assert!(err.to_string().contains("unclosed placeholder"), "{err}");

        Ok(())
    }
}
