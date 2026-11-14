//! What a task changes in the contract of an API.
//!
//! Two descriptions are compared: the one of the base and the one of the
//! task's branch. Every difference is said in a sentence and weighed by what
//! it does to a client that was written against the base:
//!
//! - **breaking**: such a client stops working. An operation is gone, a
//!   request needs something it did not need, an answer lost a field or a
//!   field changed its kind.
//! - **careful**: such a client keeps working unless it leaned on the detail.
//!   A field may now be missing or nothing, a value it never saw may come back.
//! - **safe**: only more is offered.
//!
//! The weighing follows who writes and who reads. A request is written by the
//! client, so what the service newly demands breaks it. An answer is read by
//! the client, so what the service no longer gives breaks it.

use super::spec::{self, Leaf, METHODS, Shape};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// How many differences are said. A description that changed more than this was rewritten.
const MOST: usize = 400;

/// One difference between the base and the branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    /// The operation it is about, as the branch spells it (or the base, for one that is gone).
    pub operation: String,
    /// "breaking", "careful" or "safe".
    pub level: &'static str,
    pub text: String,
}

/// Everything of one operation that a client depends on.
struct Detail {
    /// The method and the path as written.
    id: String,
    deprecated: bool,
    /// By where it goes and its name: whether it is required, and its kind.
    parameters: BTreeMap<(String, String), (bool, String)>,
    /// Whether a body must be sent, and its shape.
    body: Option<(bool, Shape)>,
    /// The shape of each answer, by status. An answer without a described body has no shape.
    answers: BTreeMap<String, Option<Shape>>,
}

/// A path with the names of its blanks taken out: `/users/{id}` and `/users/{user_id}` are the same path.
fn blank(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut inside = false;
    for sign in path.chars() {
        match sign {
            '{' => {
                inside = true;
                out.push_str("{}");
            }
            '}' => inside = false,
            _ if inside => {}
            other => out.push(other),
        }
    }
    out
}

fn details(spec: &Value) -> BTreeMap<String, Detail> {
    let mut found = BTreeMap::new();
    let Some(paths) = spec.get("paths").and_then(Value::as_object) else { return found };
    for (path, item) in paths {
        let item = spec::resolve(spec, item);
        for method in METHODS {
            let Some(operation) = item.get(method).filter(|one| one.is_object()) else { continue };
            let parameters = spec::parameters_of(spec, item, operation)
                .into_iter()
                .map(|one| {
                    let word = |name: &str| one.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
                    let (name, place) = (word("name"), word("in"));
                    let required = one.get("required").and_then(Value::as_bool).unwrap_or(false) || place == "path";
                    // The name of a blank in the path is the author's own business.
                    let name = if place == "path" { String::new() } else { name };
                    ((place, name), (required, spec::kind_of(spec, spec::parameter_schema(one))))
                })
                .collect();
            let body = spec::body_of(spec, operation).map(|(required, _, content)| {
                (required, content.get("schema").map(|schema| spec::shape(spec, schema)).unwrap_or_default())
            });
            let answers = operation
                .get("responses")
                .and_then(Value::as_object)
                .map(|all| {
                    all.iter()
                        .map(|(status, answer)| {
                            (status.clone(), spec::answer_schema(spec, answer).map(|schema| spec::shape(spec, schema)))
                        })
                        .collect()
                })
                .unwrap_or_default();
            found.insert(
                format!("{} {}", method.to_uppercase(), blank(path)),
                Detail {
                    id: format!("{} {path}", method.to_uppercase()),
                    deprecated: operation.get("deprecated").and_then(Value::as_bool).unwrap_or(false),
                    parameters,
                    body,
                    answers,
                },
            );
        }
    }
    found
}

/// How a place is said: its way from the top in code marks, or "the whole" for the top itself.
fn place(name: &str) -> String {
    if name.is_empty() { "the whole of it".into() } else { format!("`{name}`") }
}

/// Whether everything above a place is required too: only then is the place itself certain to matter.
fn certain(shape: &Shape, name: &str) -> bool {
    let mut at = name;
    loop {
        if !shape.get(at).is_some_and(|leaf| leaf.required) {
            return false;
        }
        // One step up: over a field's dot or a list's brackets.
        let up = match (at.rfind('.'), at.rfind("[]")) {
            (Some(dot), Some(list)) => dot.max(list),
            (Some(dot), None) => dot,
            (None, Some(list)) => list,
            (None, None) => return true,
        };
        at = &at[..up];
    }
}

/// Whether the thing a place is in is itself missing from the other shape: saying that thing says it all.
fn goes_with_parent(other: &Shape, name: &str) -> bool {
    name.rfind(['.', '[']).is_some_and(|up| !name[..up].is_empty() && !other.contains_key(&name[..up]))
}

fn values_lost(before: &Leaf, after: &Leaf) -> Vec<String> {
    if after.values.is_empty() {
        return Vec::new();
    }
    before.values.iter().filter(|value| !after.values.contains(value)).cloned().collect()
}

fn values_gained(before: &Leaf, after: &Leaf) -> Vec<String> {
    if before.values.is_empty() {
        return Vec::new();
    }
    after.values.iter().filter(|value| !before.values.contains(value)).cloned().collect()
}

fn quoted(values: &[String]) -> String {
    values.iter().map(|value| format!("\"{value}\"")).collect::<Vec<_>>().join(", ")
}

/// What changed in something the client writes.
fn written(before: &Shape, after: &Shape, what: &str, say: &mut dyn FnMut(&'static str, String)) {
    for (name, now) in after {
        match before.get(name) {
            None if goes_with_parent(before, name) => {}
            None if certain(after, name) => say("breaking", format!("{what} now needs {}", place(name))),
            None => say("safe", format!("{what} takes a new {}", place(name))),
            Some(was) => {
                if was.kind != now.kind {
                    say("breaking", format!("{} in {what} is now {} (it was {})", place(name), now.kind, was.kind));
                }
                if !was.required && now.required && certain(after, name) {
                    say("breaking", format!("{what} now needs {}", place(name)));
                }
                let lost = values_lost(was, now);
                if !lost.is_empty() || (was.values.is_empty() && !now.values.is_empty() && was.kind == now.kind) {
                    let which = match lost.is_empty() {
                        true => format!("takes only {}", quoted(&now.values)),
                        false => format!("no longer takes {}", quoted(&lost)),
                    };
                    say("breaking", format!("{} in {what} {which}", place(name)));
                }
                if was.nullable && !now.nullable {
                    say("breaking", format!("{} in {what} may no longer be null", place(name)));
                }
            }
        }
    }
    for name in before.keys().filter(|name| !after.contains_key(*name)) {
        if !goes_with_parent(after, name) {
            say("careful", format!("{what} no longer takes {}", place(name)));
        }
    }
}

/// What changed in something the client reads.
fn read(before: &Shape, after: &Shape, what: &str, say: &mut dyn FnMut(&'static str, String)) {
    for (name, was) in before {
        match after.get(name) {
            None => {
                if !goes_with_parent(after, name) {
                    say("breaking", format!("{what} no longer has {}", place(name)));
                }
            }
            Some(now) => {
                if was.kind != now.kind {
                    say("breaking", format!("{} in {what} is now {} (it was {})", place(name), now.kind, was.kind));
                }
                if was.required && !now.required {
                    say("careful", format!("{} may now be missing from {what}", place(name)));
                }
                if !was.nullable && now.nullable {
                    say("careful", format!("{} in {what} may now be null", place(name)));
                }
                let gained = values_gained(was, now);
                if !gained.is_empty() {
                    say("careful", format!("{} in {what} can now be {}", place(name), quoted(&gained)));
                }
            }
        }
    }
    for name in after.keys().filter(|name| !before.contains_key(*name)) {
        if !goes_with_parent(before, name) {
            say("safe", format!("{what} has a new {}", place(name)));
        }
    }
}

fn parameter(key: &(String, String)) -> String {
    match (key.0.as_str(), key.1.as_str()) {
        ("path", _) => "the blank in the path".into(),
        (place, name) => format!("the {place} parameter `{name}`"),
    }
}

fn compare_one(was: &Detail, now: &Detail, say: &mut dyn FnMut(&'static str, String)) {
    if !was.deprecated && now.deprecated {
        say("careful", "It is marked as deprecated".into());
    }
    for (key, (required, kind)) in &now.parameters {
        match was.parameters.get(key) {
            None if *required => say("breaking", format!("It now needs {}", parameter(key))),
            None => say("safe", format!("It takes a new {}", &parameter(key)[4..])),
            Some((needed, before)) => {
                if !needed && *required {
                    say("breaking", format!("It now needs {}", parameter(key)));
                }
                if before != kind {
                    say("breaking", format!("{} is now {kind} (it was {before})", capital(&parameter(key))));
                }
            }
        }
    }
    for key in was.parameters.keys().filter(|key| !now.parameters.contains_key(*key)) {
        say("careful", format!("It no longer takes {}", parameter(key)));
    }

    match (&was.body, &now.body) {
        (None, Some((true, _))) => say("breaking", "It now needs a body".into()),
        (None, Some((false, _))) => say("safe", "It takes a body now".into()),
        (Some(_), None) => say("careful", "It no longer takes a body".into()),
        (Some((needed, before)), Some((required, after))) => {
            if !needed && *required {
                say("breaking", "It now needs a body".into());
            }
            written(before, after, "the body", say);
        }
        (None, None) => {}
    }

    for (status, before) in &was.answers {
        match now.answers.get(status) {
            None if status.starts_with('2') => say("breaking", format!("It no longer answers {status}")),
            None => say("careful", format!("It no longer answers {status}")),
            Some(after) => match (before, after) {
                (Some(before), Some(after)) => read(before, after, &format!("the {status} answer"), say),
                (Some(_), None) => say("breaking", format!("The {status} answer no longer says what it holds")),
                _ => {}
            },
        }
    }
    for status in now.answers.keys().filter(|status| !was.answers.contains_key(*status)) {
        say("careful", format!("It can now answer {status}"));
    }
}

fn capital(text: &str) -> String {
    let mut signs = text.chars();
    match signs.next() {
        Some(first) => first.to_uppercase().chain(signs).collect(),
        None => String::new(),
    }
}

/// Every difference between two descriptions, the heaviest first, and within a weight in the order of the paths.
pub fn compare(base: &Value, branch: &Value) -> Vec<Change> {
    let (before, after) = (details(base), details(branch));
    let mut changes: Vec<Change> = Vec::new();
    for (key, was) in &before {
        match after.get(key) {
            None => changes.push(Change { operation: was.id.clone(), level: "breaking", text: "It is gone".into() }),
            Some(now) => {
                let mut said: Vec<(&'static str, String)> = Vec::new();
                compare_one(was, now, &mut |level, text| said.push((level, text)));
                // The same sentence can come from two parts of one schema.
                said.dedup();
                changes.extend(said.into_iter().map(|(level, text)| Change {
                    operation: now.id.clone(),
                    level,
                    text: capital(&text),
                }));
            }
        }
    }
    for (key, now) in &after {
        if !before.contains_key(key) {
            changes.push(Change { operation: now.id.clone(), level: "safe", text: "It is new".into() });
        }
    }
    let weight = |level: &str| match level {
        "breaking" => 0,
        "careful" => 1,
        _ => 2,
    };
    changes.sort_by_key(|change| weight(change.level));
    changes.truncate(MOST);
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(paths: Value) -> Value {
        json!({ "openapi": "3.0.3", "paths": paths })
    }

    fn order(properties: Value, required: Value) -> Value {
        json!({ "type": "object", "required": required, "properties": properties })
    }

    fn orders(method: &str, body: Option<Value>, answer: Value) -> Value {
        let mut operation =
            json!({ "responses": { "200": { "content": { "application/json": { "schema": answer } } } } });
        if let Some(body) = body {
            operation["requestBody"] = json!({ "content": { "application/json": { "schema": body } } });
        }
        spec(json!({ "/orders": { method: operation } }))
    }

    fn told(base: &Value, branch: &Value) -> Vec<(&'static str, String)> {
        compare(base, branch).into_iter().map(|change| (change.level, change.text)).collect()
    }

    #[test]
    fn two_descriptions_that_are_the_same_differ_in_nothing() {
        let one = orders("get", None, order(json!({ "id": { "type": "integer" } }), json!(["id"])));
        assert!(compare(&one, &one).is_empty());
    }

    #[test]
    fn an_operation_that_is_gone_breaks_and_a_new_one_is_safe() {
        let base =
            spec(json!({ "/orders": { "get": { "responses": {} } }, "/users/{id}": { "get": { "responses": {} } } }));
        let branch = spec(
            json!({ "/users/{user_id}": { "get": { "responses": {} } }, "/carts": { "post": { "responses": {} } } }),
        );
        let changes = compare(&base, &branch);
        assert_eq!(changes.len(), 2, "a blank in a path may be renamed: {changes:?}");
        assert_eq!(
            (changes[0].operation.as_str(), changes[0].level, changes[0].text.as_str()),
            ("GET /orders", "breaking", "It is gone")
        );
        assert_eq!(
            (changes[1].operation.as_str(), changes[1].level, changes[1].text.as_str()),
            ("POST /carts", "safe", "It is new")
        );
    }

    #[test]
    fn what_a_request_newly_needs_breaks() {
        let base = orders(
            "post",
            Some(order(json!({ "sku": { "type": "string" }, "note": { "type": "string" } }), json!(["sku"]))),
            json!({}),
        );
        let branch = orders(
            "post",
            Some(order(
                json!({ "sku": { "type": "string" }, "count": { "type": "integer" }, "gift": { "type": "boolean" } }),
                json!(["sku", "count"]),
            )),
            json!({}),
        );
        let said = told(&base, &branch);
        assert!(said.contains(&("breaking", "The body now needs `count`".into())), "{said:?}");
        assert!(said.contains(&("safe", "The body takes a new `gift`".into())), "{said:?}");
        assert!(said.contains(&("careful", "The body no longer takes `note`".into())), "{said:?}");
        assert_eq!(said[0].0, "breaking", "the heaviest is said first");
    }

    #[test]
    fn a_new_required_field_inside_something_optional_does_not_break() {
        let address = |required: Value| {
            order(
                json!({ "address": order(json!({ "city": { "type": "string" }, "zip": { "type": "string" } }), required) }),
                json!([]),
            )
        };
        let base = orders("post", Some(order(json!({}), json!([]))), json!({}));
        let branch = orders("post", Some(address(json!(["zip"]))), json!({}));
        let said = told(&base, &branch);
        assert!(said.iter().all(|(level, _)| *level == "safe"), "{said:?}");
        assert_eq!(said.len(), 1, "the new object is said once, not field by field: {said:?}");
    }

    #[test]
    fn what_an_answer_no_longer_gives_breaks() {
        let base = orders(
            "get",
            None,
            order(
                json!({ "id": { "type": "integer" }, "total": { "type": "number" }, "state": { "type": "string", "enum": ["open"] }, "buyer": order(json!({ "name": { "type": "string" } }), json!([])) }),
                json!(["id", "total"]),
            ),
        );
        let branch = orders(
            "get",
            None,
            order(
                json!({ "id": { "type": "string" }, "total": { "type": "number", "nullable": true }, "state": { "type": "string", "enum": ["open", "held"] }, "placed": { "type": "string" } }),
                json!(["id"]),
            ),
        );
        let said = told(&base, &branch);
        assert!(
            said.contains(&("breaking", "`id` in the 200 answer is now string (it was integer)".into())),
            "{said:?}"
        );
        assert!(said.contains(&("breaking", "The 200 answer no longer has `buyer`".into())), "{said:?}");
        assert!(!said.iter().any(|(_, text)| text.contains("buyer.name")), "what was under it goes with it: {said:?}");
        assert!(said.contains(&("careful", "`total` may now be missing from the 200 answer".into())), "{said:?}");
        assert!(said.contains(&("careful", "`total` in the 200 answer may now be null".into())), "{said:?}");
        assert!(said.contains(&("careful", "`state` in the 200 answer can now be \"held\"".into())), "{said:?}");
        assert!(said.contains(&("safe", "The 200 answer has a new `placed`".into())), "{said:?}");
    }

    #[test]
    fn parameters_and_statuses_are_weighed_too() {
        let operation = |parameters: Value, responses: Value| {
            spec(json!({ "/orders": { "get": { "parameters": parameters, "responses": responses } } }))
        };
        let base = operation(
            json!([{ "name": "page", "in": "query", "schema": { "type": "integer" } }, { "name": "sort", "in": "query", "schema": { "type": "string" } }]),
            json!({ "200": {}, "404": {} }),
        );
        let branch = operation(
            json!([{ "name": "page", "in": "query", "required": true, "schema": { "type": "string" } }, { "name": "shop", "in": "header", "required": true, "schema": { "type": "string" } }]),
            json!({ "201": {} }),
        );
        let said = told(&base, &branch);
        for expected in [
            ("breaking", "It now needs the query parameter `page`"),
            ("breaking", "The query parameter `page` is now string (it was integer)"),
            ("breaking", "It now needs the header parameter `shop`"),
            ("careful", "It no longer takes the query parameter `sort`"),
            ("breaking", "It no longer answers 200"),
            ("careful", "It no longer answers 404"),
            ("careful", "It can now answer 201"),
        ] {
            assert!(said.contains(&(expected.0, expected.1.to_string())), "{expected:?} is not in {said:?}");
        }
    }

    #[test]
    fn a_narrower_list_of_values_in_a_request_breaks() {
        let state = |values: Value| {
            orders("post", Some(order(json!({ "state": { "type": "string", "enum": values } }), json!([]))), json!({}))
        };
        let said = told(&state(json!(["open", "held", "paid"])), &state(json!(["open"])));
        assert_eq!(said, [("breaking", "`state` in the body no longer takes \"held\", \"paid\"".to_string())]);
    }
}
