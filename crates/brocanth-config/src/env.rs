//! The environment as a layer.
//!
//! The environment is not a file format and it does not get to behave like one.
//! Three questions have to be answered before it can be a source at all, and
//! each answer is a decision rather than a convention borrowed from elsewhere.
//!
//! # 1. How a nested key is spelled
//!
//! `<SNAKE_UPPER>_<SECTION>__<KEY>`, with `__` between path segments:
//!
//! ```text
//! MY_APP_SERVER__PORT                 → [server]        port
//! MY_APP_SERVER__POOL__MAX_SIZE       → [server.pool]   max_size
//! MY_APP_REQUEST_ID__PREFIX           → [request_id]    prefix
//! ```
//!
//! The prefix is [`AppName::env_var`](crate::AppName::env_var), unchanged, so
//! there is still exactly one spelling of the application's name and
//! `MY_APP_ENV`, `MY_APP_CONFIG` and `MY_APP_SERVER__PORT` are visibly the same
//! family.
//!
//! The separator is `__` and not `_` because every section and key name in this
//! crate is `snake_case`, so a single underscore is already taken:
//! `MY_APP_REQUEST_ID_PREFIX` could be `request.id.prefix`, `request_id.prefix`
//! or `request.id_prefix`, and no rule decides between them. `__` has no such
//! problem.
//!
//! One consequence falls out and it is a useful one: **a configuration variable
//! always contains `__`**, because it always names a section and a key. So
//! `MY_APP_ENV` and `MY_APP_CONFIG` — the loader's own two variables — are not
//! configuration keys by construction rather than by a reserved-word list, and
//! an unrelated `MY_APP_HOME` in the environment is ignored instead of becoming
//! a top-level scalar that the section check would then reject.
//!
//! # 2. How a value is typed when everything arrives as a string
//!
//! Inferred, with one escape:
//!
//! | text | becomes |
//! |---|---|
//! | `true`, `false` | boolean |
//! | `-12`, `0`, `9000` | integer |
//! | `1.5`, `2e3` | float |
//! | `"true"` (with the quotes) | the string `true` |
//! | anything else | string |
//!
//! Inference is the only option that does not make the common case wrong. A
//! value that stayed a string would fail to deserialise into every `u16`,
//! `bool` and `usize` a plugin declares, and the fix — a
//! `deserialize_with` on every numeric field of every plugin — would be a tax
//! on every plugin author to serve one source. The cost of inferring is an
//! application that wants the literal string `"true"`, and the quoting escape
//! answers that in one keystroke.
//!
//! `true` and `false` are matched exactly, lower case. `TRUE` is the string
//! `TRUE`, because an environment that shouts is common and a rule that
//! lower-cases silently would turn the string `"TRUE"` into a boolean behind
//! the operator's back.
//!
//! # 3. How an array is spelled
//!
//! By index: `MY_APP_DB__HOSTS__0`, `MY_APP_DB__HOSTS__1`. A segment of digits
//! is an array index.
//!
//! The obvious alternative, a comma-separated list, cannot spell a value that
//! contains a comma and has no way to say "one element, and it is empty".
//! Indices have neither problem and they reuse the separator that is already
//! there. The indices must run from 0 with no gap; a gap is an error naming the
//! index that is missing, because a silently shortened list is the failure
//! class this crate is built to avoid.
//!
//! # What this does to the merge rule
//!
//! Nothing, and that is worth stating. The whole environment becomes **one
//! layer** before anything merges, so `tables recurse, scalars and arrays are
//! replaced whole` applies to it unchanged:
//!
//! * `MY_APP_SERVER__PORT` alone does not disturb `[server] host`, because the
//!   `server` table recurses.
//! * `MY_APP_DB__HOSTS__0` alone **does** replace a three-element `hosts` from
//!   a file with a one-element array, because the env layer finishes building
//!   its own `hosts` array first and then that array replaces the lower one
//!   whole. That is the rule holding rather than an exception to it, and it is
//!   tested.

use std::collections::BTreeMap;

use crate::value::{Provenance, Table, Value, ValueKind};
use crate::AppName;

/// A variable this layer cannot turn into a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvError {
    /// The variable that could not be used.
    pub var: String,
    /// What is wrong with it.
    pub message: String,
}

impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "${}: {}", self.var, self.message)
    }
}

impl std::error::Error for EnvError {}

/// The variables of one application, turned into one layer.
///
/// Returns the layer and the names of the variables that contributed, in the
/// order the dump should list them.
///
/// # Errors
///
/// [`EnvError`] when a variable names a path that contradicts another one, or
/// when an array has a gap in its indices.
pub fn layer(app: &AppName, vars: &[(String, String)]) -> Result<(Value, Vec<String>), EnvError> {
    let prefix = format!("{}_", app.as_str().to_ascii_uppercase());

    // Collected first and sorted, so two variables that disagree produce the
    // same error whatever order the process environment happened to list them
    // in.
    let mut claimed: Vec<(Vec<String>, String, String)> = Vec::new();
    for (name, raw) in vars {
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if !rest.contains("__") {
            continue;
        }
        let path: Vec<String> = rest.split("__").map(|s| s.to_ascii_lowercase()).collect();
        if path.iter().any(String::is_empty) {
            return Err(EnvError {
                var: name.clone(),
                message: "a path segment is empty; `__` separates one name from the next".into(),
            });
        }
        claimed.push((path, name.clone(), raw.clone()));
    }
    claimed.sort_by(|a, b| a.0.cmp(&b.0));

    let mut root = Node::default();
    for (path, name, raw) in &claimed {
        root.insert(path, name, raw)?;
    }

    let used: Vec<String> = claimed.into_iter().map(|(_, name, _)| name).collect();
    let value = root.finish(&Provenance::inline("environment"))?;
    Ok((value, used))
}

/// A tree built from paths before it knows whether a level is a table or an
/// array. The decision needs every variable at that level, so it is taken in
/// [`Node::finish`] rather than on the way in.
#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    leaf: Option<(String, String)>,
}

impl Node {
    fn insert(&mut self, path: &[String], var: &str, raw: &str) -> Result<(), EnvError> {
        let Some((head, tail)) = path.split_first() else {
            if self.leaf.is_some() || !self.children.is_empty() {
                return Err(EnvError {
                    var: var.to_string(),
                    message: "this names a value, and another variable names keys under it".into(),
                });
            }
            self.leaf = Some((var.to_string(), raw.to_string()));
            return Ok(());
        };
        let child = self.children.entry(head.clone()).or_default();
        if let Some((other, _)) = child.leaf.as_ref() {
            if !tail.is_empty() {
                return Err(EnvError {
                    var: var.to_string(),
                    message: format!("${other} already names this as a value, not a table"),
                });
            }
        }
        child.insert(tail, var, raw)
    }

    fn finish(self, parent: &Provenance) -> Result<Value, EnvError> {
        if let Some((var, raw)) = self.leaf {
            let from = Provenance::env(&var);
            return Ok(Value::new(infer(&raw), from));
        }

        // Every child name is a number: this level is an array.
        let all_numeric =
            !self.children.is_empty() && self.children.keys().all(|k| k.parse::<usize>().is_ok());

        if all_numeric {
            let mut indices: Vec<usize> = self
                .children
                .keys()
                .map(|k| k.parse::<usize>().expect("checked above"))
                .collect();
            indices.sort_unstable();
            for (expected, got) in indices.iter().enumerate() {
                if expected != *got {
                    let any = self
                        .children
                        .values()
                        .find_map(Node::first_var)
                        .unwrap_or_default();
                    return Err(EnvError {
                        var: any,
                        message: format!(
                            "an array is spelled with every index from 0; index {expected} is missing"
                        ),
                    });
                }
            }
            let mut items = Vec::with_capacity(self.children.len());
            let mut ordered: Vec<(usize, Node)> = self
                .children
                .into_iter()
                .map(|(k, v)| (k.parse::<usize>().expect("checked above"), v))
                .collect();
            ordered.sort_by_key(|(i, _)| *i);
            for (_, node) in ordered {
                items.push(node.finish(parent)?);
            }
            return Ok(Value::new(ValueKind::Array(items), parent.clone()));
        }

        let mut table = Table::open();
        for (key, node) in self.children {
            table.insert(key, node.finish(parent)?);
        }
        Ok(Value::new(ValueKind::Table(table), parent.clone()))
    }

    fn first_var(&self) -> Option<String> {
        if let Some((var, _)) = &self.leaf {
            return Some(var.clone());
        }
        self.children.values().find_map(Node::first_var)
    }
}

/// Types one variable's text. See the module documentation for the rule and
/// the reason.
fn infer(raw: &str) -> ValueKind {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return ValueKind::String(raw[1..raw.len() - 1].to_string());
    }
    match raw {
        "true" => return ValueKind::Boolean(true),
        "false" => return ValueKind::Boolean(false),
        _ => {}
    }
    if let Ok(i) = raw.parse::<i64>() {
        // `parse::<i64>` accepts a leading `+`; keep it a number, it is one.
        return ValueKind::Integer(i);
    }
    if let Ok(f) = raw.parse::<f64>() {
        if f.is_finite() {
            return ValueKind::Float(f);
        }
    }
    ValueKind::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn build(pairs: &[(&str, &str)]) -> Value {
        layer(&AppName::new("my_app"), &vars(pairs)).unwrap().0
    }

    #[test]
    fn a_double_underscore_separates_a_section_from_a_key() {
        let v = build(&[
            ("MY_APP_SERVER__PORT", "8080"),
            ("MY_APP_SERVER__POOL__MAX_SIZE", "20"),
            ("MY_APP_REQUEST_ID__PREFIX", "web"),
        ]);
        assert_eq!(v.at("server.port").unwrap().as_integer(), Some(8080));
        assert_eq!(v.at("server.pool.max_size").unwrap().as_integer(), Some(20));
        assert_eq!(v.at("request_id.prefix").unwrap().as_str(), Some("web"));
    }

    #[test]
    fn the_loaders_own_variables_are_not_configuration_keys() {
        // They have no `__`, so they name no section and no key. Nothing is
        // reserved; the spelling rule does the work.
        let v = build(&[
            ("MY_APP_ENV", "production"),
            ("MY_APP_CONFIG", "/srv/app.toml"),
            ("MY_APP_HOME", "/srv"),
            ("OTHER_APP_SERVER__PORT", "1"),
            ("MY_APP_SERVER__PORT", "80"),
        ]);
        let table = v.as_table().unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(v.at("server.port").unwrap().as_integer(), Some(80));
    }

    #[test]
    fn a_value_is_typed_by_what_it_says() {
        let v = build(&[
            ("MY_APP_X__B", "true"),
            ("MY_APP_X__SHOUTED", "TRUE"),
            ("MY_APP_X__I", "-12"),
            ("MY_APP_X__F", "1.5"),
            ("MY_APP_X__S", "warn"),
            ("MY_APP_X__QUOTED", "\"true\""),
            ("MY_APP_X__VERSIONISH", "1.2.3"),
        ]);
        assert_eq!(v.at("x.b").unwrap().as_bool(), Some(true));
        assert_eq!(v.at("x.shouted").unwrap().as_str(), Some("TRUE"));
        assert_eq!(v.at("x.i").unwrap().as_integer(), Some(-12));
        assert_eq!(v.at("x.f").unwrap().as_float(), Some(1.5));
        assert_eq!(v.at("x.s").unwrap().as_str(), Some("warn"));
        assert_eq!(v.at("x.quoted").unwrap().as_str(), Some("true"));
        assert_eq!(v.at("x.versionish").unwrap().as_str(), Some("1.2.3"));
    }

    #[test]
    fn an_array_is_spelled_with_indices_and_keeps_their_order() {
        let v = build(&[
            ("MY_APP_DB__HOSTS__1", "b"),
            ("MY_APP_DB__HOSTS__0", "a"),
            ("MY_APP_DB__HOSTS__2", "c"),
        ]);
        let hosts = v.at("db.hosts").unwrap().as_array().unwrap();
        let got: Vec<&str> = hosts.iter().map(|h| h.as_str().unwrap()).collect();
        assert_eq!(got, vec!["a", "b", "c"]);
    }

    #[test]
    fn a_gap_in_the_indices_is_an_error_naming_the_missing_one() {
        let err = layer(
            &AppName::new("my_app"),
            &vars(&[("MY_APP_DB__HOSTS__0", "a"), ("MY_APP_DB__HOSTS__2", "c")]),
        )
        .unwrap_err();
        assert!(err.message.contains("index 1 is missing"), "{err}");
    }

    #[test]
    fn a_variable_that_names_a_value_and_a_table_at_once_is_an_error() {
        let err = layer(
            &AppName::new("my_app"),
            &vars(&[("MY_APP_DB__POOL", "5"), ("MY_APP_DB__POOL__SIZE", "5")]),
        )
        .unwrap_err();
        assert!(err.message.contains("not a table"), "{err}");
    }

    #[test]
    fn every_leaf_names_the_variable_it_came_from() {
        let v = build(&[("MY_APP_SERVER__PORT", "8080")]);
        assert_eq!(
            v.at("server.port").unwrap().provenance().to_string(),
            "$MY_APP_SERVER__PORT"
        );
    }

    #[test]
    fn the_variables_that_contributed_are_reported_in_name_order() {
        let (_, used) = layer(
            &AppName::new("MyApp"),
            &vars(&[
                ("MY_APP_SERVER__PORT", "1"),
                ("MY_APP_ENV", "test"),
                ("MY_APP_LOGGER__LEVEL", "warn"),
            ]),
        )
        .unwrap();
        assert_eq!(used, vec!["MY_APP_LOGGER__LEVEL", "MY_APP_SERVER__PORT"]);
    }
}
