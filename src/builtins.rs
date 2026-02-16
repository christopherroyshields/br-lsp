use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::extract::ParamKind;

#[derive(Debug, Deserialize)]
pub struct BuiltinFunction {
    pub name: String,
    pub documentation: Option<String>,
    pub params: Vec<BuiltinParam>,
}

#[derive(Debug, Deserialize)]
pub struct BuiltinParam {
    pub name: String,
    pub documentation: Option<String>,
}

impl BuiltinParam {
    /// Infer the expected parameter type from naming conventions:
    /// - `$` suffix → string
    /// - `MAT` prefix → array
    /// - Literal string params (e.g. `"MD5"`) → None (skip type checking)
    /// - Ambiguous params (e.g. arrays without type indicator) → None
    pub fn kind(&self) -> Option<ParamKind> {
        // Skip literal string params ("MD5", "KB")
        if self.name.starts_with('"') {
            return None;
        }
        // Strip wrapper chars for analysis
        let stripped = self.name.replace(['[', ']', '<', '>', '*', '^'], "");
        let stripped = stripped.trim();
        let is_mat = stripped.starts_with("MAT ") || stripped.starts_with("mat ");
        let is_string = stripped.ends_with('$');

        if is_mat {
            if is_string {
                return Some(ParamKind::StringArray);
            }
            // MAT without $ — only NumericArray if explicitly "numeric" in name
            let inner = stripped
                .trim_start_matches("MAT ")
                .trim_start_matches("mat ")
                .trim();
            if inner.to_ascii_lowercase().contains("numeric") {
                return Some(ParamKind::NumericArray);
            }
            // Ambiguous array (could be numeric or string)
            return None;
        }

        if is_string {
            return Some(ParamKind::String);
        }

        // Non-MAT, non-$ — check for known ambiguous patterns
        let lower = stripped.to_ascii_lowercase();
        if lower.contains("array") || lower == "date" || lower == "argument" {
            return None;
        }

        Some(ParamKind::Numeric)
    }
}

static BUILTINS: LazyLock<HashMap<String, Vec<BuiltinFunction>>> = LazyLock::new(|| {
    let json = include_str!("builtins.json");
    let functions: Vec<BuiltinFunction> =
        serde_json::from_str(json).expect("failed to parse builtins.json");
    let mut map: HashMap<String, Vec<BuiltinFunction>> = HashMap::new();
    for func in functions {
        let key = func.name.to_ascii_lowercase();
        map.entry(key).or_default().push(func);
    }
    map
});

pub fn lookup(name: &str) -> &'static [BuiltinFunction] {
    BUILTINS
        .get(&name.to_ascii_lowercase())
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

impl BuiltinFunction {
    pub fn format_signature(&self) -> String {
        if self.params.is_empty() {
            self.name.clone()
        } else {
            let params: Vec<&str> = self.params.iter().map(|p| p.name.as_str()).collect();
            format!("{}({})", self.name, params.join(", "))
        }
    }

    pub fn format_signature_with_offsets(&self) -> (String, Vec<[u32; 2]>) {
        if self.params.is_empty() {
            return (self.name.clone(), Vec::new());
        }

        let mut label = self.name.clone();
        label.push('(');
        let mut offsets = Vec::with_capacity(self.params.len());

        for (i, param) in self.params.iter().enumerate() {
            if i > 0 {
                label.push_str(", ");
            }
            let start = label.len() as u32;
            label.push_str(&param.name);
            let end = label.len() as u32;
            offsets.push([start, end]);
        }
        label.push(')');

        (label, offsets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_val() {
        let results = lookup("Val");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Val");
    }

    #[test]
    fn lookup_case_insensitive() {
        let results = lookup("VAL");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Val");
    }

    #[test]
    fn lookup_overloaded() {
        let results = lookup("Decrypt$");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn lookup_missing() {
        let results = lookup("nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn format_signature_no_params() {
        let results = lookup("Bell");
        assert_eq!(results[0].format_signature(), "Bell");
    }

    #[test]
    fn format_signature_with_params() {
        let results = lookup("Str$");
        assert_eq!(results[0].format_signature(), "Str$(<number>)");
    }

    #[test]
    fn format_signature_offsets() {
        let results = lookup("Cnvrt$");
        let (label, offsets) = results[0].format_signature_with_offsets();
        assert_eq!(label, "Cnvrt$(<Spec$>, <Number>)");
        assert_eq!(offsets.len(), 2);
        // Verify offsets point to the right substrings
        let spec = &label[offsets[0][0] as usize..offsets[0][1] as usize];
        assert_eq!(spec, "<Spec$>");
        let num = &label[offsets[1][0] as usize..offsets[1][1] as usize];
        assert_eq!(num, "<Number>");
    }
}
