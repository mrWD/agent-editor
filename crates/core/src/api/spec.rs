//! Reading a description of an API.
//!
//! Two ways of writing one are read: OpenAPI 3 and the older Swagger 2. Both
//! are a tree of JSON (or YAML) in which a part may stand somewhere else and be
//! pointed at with `$ref`. Only pointers into the same document are followed.
//!
//! Out of the tree come the things the interface shows: every operation with
//! what it takes, a body to start a request from, and, for the comparison of
//! two descriptions, the shape of what goes in and comes out.

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The methods a path can offer, in the order they are listed.
pub const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];
/// How far a pointer is followed to another pointer.
const MOST_HOPS: usize = 16;
/// How deep a shape or an example goes. A shape that holds itself ends here.
const MOST_DEPTH: usize = 8;
/// How many places one shape may have. Past this the rest is not looked at.
const MOST_PLACES: usize = 2000;

static NOTHING: Value = Value::Null;

/// Reads a description from the bytes of a file or an answer. JSON is tried first, then YAML.
pub fn read(bytes: &[u8]) -> Result<Value> {
    let spec: Value = match serde_json::from_slice(bytes) {
        Ok(spec) => spec,
        Err(_) => match serde_yaml_ng::from_slice(bytes) {
            Ok(spec) => spec,
            Err(_) => bail!("It is neither JSON nor YAML"),
        },
    };
    if !spec.is_object() {
        bail!("It is neither JSON nor YAML");
    }
    let named = spec.get("openapi").is_some() || spec.get("swagger").is_some();
    if !named || !spec.get("paths").is_some_and(Value::is_object) {
        bail!("It is not an OpenAPI or Swagger description: it has no version and no paths");
    }
    Ok(spec)
}

/// The thing a node stands for: itself, or what its `$ref` points at.
pub fn resolve<'a>(spec: &'a Value, mut node: &'a Value) -> &'a Value {
    for _ in 0..MOST_HOPS {
        let Some(pointer) = node.get("$ref").and_then(Value::as_str) else { return node };
        match pointer.strip_prefix('#').and_then(|inside| spec.pointer(inside)) {
            Some(found) => node = found,
            None => return &NOTHING,
        }
    }
    &NOTHING
}

fn text(node: &Value, name: &str) -> String {
    node.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn list<'a>(node: &'a Value, name: &str) -> &'a [Value] {
    node.get(name).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The name of the description and its version, as its author wrote them.
pub fn title(spec: &Value) -> (String, String) {
    let info = spec.get("info").unwrap_or(&NOTHING);
    (text(info, "title"), text(info, "version"))
}

/// What stands before every path of the description: the path of its first server, or Swagger's `basePath`.
/// Empty when the paths start at the root.
pub fn prefix(spec: &Value) -> String {
    let server = list(spec, "servers").first().map(|server| text(server, "url")).unwrap_or_default();
    let path = match server.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |at| &rest[at..]),
        None if server.starts_with('/') => server.as_str(),
        None => "",
    };
    let path = if path.is_empty() { text(spec, "basePath") } else { path.to_string() };
    let path = path.trim_end_matches('/');
    if path.starts_with('/') && !path.contains(['{', '?', '#', ' ']) { path.to_string() } else { String::new() }
}

/// What kind of value a schema describes, in a word or two: "integer", "string (uuid)", "object[]".
pub fn kind_of(spec: &Value, schema: &Value) -> String {
    kind_at(spec, schema, 0)
}

fn kind_at(spec: &Value, schema: &Value, depth: usize) -> String {
    let schema = resolve(spec, schema);
    let kind = match schema.get("type") {
        Some(Value::String(one)) => one.clone(),
        // OpenAPI 3.1 writes "string or nothing" as a list of two.
        Some(Value::Array(many)) => {
            many.iter().filter_map(Value::as_str).filter(|kind| *kind != "null").collect::<Vec<_>>().join(" or ")
        }
        _ if schema.get("properties").is_some() || schema.get("allOf").is_some() => "object".into(),
        _ if schema.get("items").is_some() => "array".into(),
        _ if schema.get("enum").is_some() => "string".into(),
        _ if schema.get("oneOf").is_some() || schema.get("anyOf").is_some() => "one of several".into(),
        _ => "anything".into(),
    };
    if kind == "array" && depth < MOST_DEPTH {
        return format!("{}[]", kind_at(spec, schema.get("items").unwrap_or(&NOTHING), depth + 1));
    }
    match schema.get("format").and_then(Value::as_str) {
        Some(format) => format!("{kind} ({format})"),
        None => kind,
    }
}

fn nullable(schema: &Value) -> bool {
    schema.get("nullable").and_then(Value::as_bool).unwrap_or(false)
        || list(schema, "type").iter().any(|kind| kind == "null")
        || ["oneOf", "anyOf"]
            .iter()
            .any(|name| list(schema, name).iter().any(|part| part.get("type") == Some(&json!("null"))))
}

/// One place in a shape: a field, an item of a list, or the whole.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Leaf {
    pub kind: String,
    /// Whether the thing that holds it must have it.
    pub required: bool,
    /// Whether it may be there and be nothing.
    pub nullable: bool,
    /// The values it may take, when they are listed.
    pub values: Vec<String>,
}

/// Every place of a value, by its way from the top: `user.address.city`, `items[].price`.
/// The whole value is the empty name.
pub type Shape = BTreeMap<String, Leaf>;

/// The shape of what a schema describes.
pub fn shape(spec: &Value, schema: &Value) -> Shape {
    let mut out = Shape::new();
    walk(spec, schema, "", true, false, 0, &mut out);
    out
}

fn said(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `loose` is for one of several alternatives: nothing in it is certain to be there.
fn walk(spec: &Value, schema: &Value, at: &str, required: bool, loose: bool, depth: usize, out: &mut Shape) {
    let schema = resolve(spec, schema);
    if depth > MOST_DEPTH || out.len() > MOST_PLACES || schema.is_null() {
        return;
    }
    for part in list(schema, "allOf") {
        walk(spec, part, at, required, loose, depth + 1, out);
    }
    for part in list(schema, "oneOf").iter().chain(list(schema, "anyOf")) {
        if part.get("type") != Some(&json!("null")) {
            walk(spec, part, at, required, true, depth + 1, out);
        }
    }
    out.insert(
        at.to_string(),
        Leaf {
            kind: kind_of(spec, schema),
            required: required && !loose,
            nullable: nullable(schema),
            values: list(schema, "enum").iter().map(said).collect(),
        },
    );
    let needed = list(schema, "required");
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, child) in properties {
            let place = if at.is_empty() { name.clone() } else { format!("{at}.{name}") };
            let must = !loose && needed.iter().any(|one| one == name);
            walk(spec, child, &place, must, loose, depth + 1, out);
        }
    }
    if let Some(items) = schema.get("items") {
        walk(spec, items, &format!("{at}[]"), true, loose, depth + 1, out);
    }
}

/// A value that fits a schema, to start a request from: the author's own example where there is one.
pub fn sample(spec: &Value, schema: &Value) -> Value {
    sample_at(spec, schema, 0)
}

fn sample_at(spec: &Value, schema: &Value, depth: usize) -> Value {
    let schema = resolve(spec, schema);
    if let Some(given) = schema.get("example").or_else(|| schema.get("default")).or_else(|| schema.get("const")) {
        return given.clone();
    }
    if let Some(first) = list(schema, "examples").first().or_else(|| list(schema, "enum").first()) {
        return first.clone();
    }
    if depth > MOST_DEPTH {
        return Value::Null;
    }
    let mut whole = Map::new();
    for part in list(schema, "allOf") {
        if let Value::Object(fields) = sample_at(spec, part, depth + 1) {
            whole.extend(fields);
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, child) in properties {
            // What only the service writes is not for a request.
            if resolve(spec, child).get("readOnly").and_then(Value::as_bool) != Some(true) {
                whole.insert(name.clone(), sample_at(spec, child, depth + 1));
            }
        }
    }
    if !whole.is_empty() || schema.get("properties").is_some() {
        return Value::Object(whole);
    }
    let alternatives = list(schema, "oneOf").iter().chain(list(schema, "anyOf"));
    if let Some(part) = alternatives.clone().find(|part| part.get("type") != Some(&json!("null"))) {
        return sample_at(spec, part, depth + 1);
    }
    let kind = kind_of(spec, schema);
    let (kind, format) = match kind.split_once(" (") {
        Some((kind, format)) => (kind, format.trim_end_matches(')')),
        None => (kind.as_str(), ""),
    };
    match (kind.split(" or ").next().unwrap_or_default(), format) {
        (kind, _) if kind.ends_with("[]") => {
            json!([sample_at(spec, schema.get("items").unwrap_or(&NOTHING), depth + 1)])
        }
        ("string", "date-time") => json!("2026-01-01T00:00:00Z"),
        ("string", "date") => json!("2026-01-01"),
        ("string", "uuid") => json!("00000000-0000-0000-0000-000000000000"),
        ("string", "email") => json!("user@example.com"),
        ("string", "uri" | "url") => json!("https://example.com"),
        ("string", _) => json!("string"),
        ("integer", _) => {
            schema.get("minimum").filter(|least| least.is_i64() || least.is_u64()).cloned().unwrap_or(json!(0))
        }
        ("number", _) => schema.get("minimum").filter(|least| least.is_number()).cloned().unwrap_or(json!(0)),
        ("boolean", _) => json!(false),
        ("object", _) => json!({}),
        _ => Value::Null,
    }
}

/// One thing an operation takes beside its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Parameter {
    pub name: String,
    /// "path", "query", "header" or "cookie".
    #[serde(rename = "in")]
    pub place: String,
    pub required: bool,
    pub kind: String,
    /// A value to start from. Empty when the description gives none.
    pub example: String,
    pub about: String,
}

/// The body an operation takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Body {
    /// Its content type, such as `application/json`.
    pub kind: String,
    pub required: bool,
    /// A body to start from, as text.
    pub example: String,
}

/// One thing the API can be asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    /// The method and the path, the way it is said: `GET /users/{id}`.
    pub id: String,
    pub method: String,
    pub path: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub deprecated: bool,
    pub parameters: Vec<Parameter>,
    pub body: Option<Body>,
    /// The statuses it says it answers with.
    pub responses: Vec<String>,
    /// "new" or "changed" against the base, when the two were compared.
    pub change: Option<&'static str>,
}

/// The parameters of one operation: those of its path, with its own in place of the ones they repeat.
/// Swagger's body and form fields are not among them.
pub fn parameters_of<'a>(spec: &'a Value, item: &'a Value, operation: &'a Value) -> Vec<&'a Value> {
    let mut found: Vec<&Value> = Vec::new();
    for one in list(item, "parameters").iter().chain(list(operation, "parameters")) {
        let one = resolve(spec, one);
        let (name, place) = (one.get("name"), one.get("in"));
        if name.is_none() || matches!(place.and_then(Value::as_str), Some("body" | "formData") | None) {
            continue;
        }
        found.retain(|known| known.get("name") != name || known.get("in") != place);
        found.push(one);
    }
    found
}

/// The schema of a parameter: its own in OpenAPI 3, the parameter itself in Swagger 2.
pub fn parameter_schema(parameter: &Value) -> &Value {
    parameter.get("schema").unwrap_or(parameter)
}

/// The content of the kind a person would write by hand: JSON when it is offered, else the first.
fn content(node: &Value) -> Option<(&String, &Value)> {
    let kinds = node.get("content")?.as_object()?;
    kinds.iter().find(|(kind, _)| kind.contains("json")).or_else(|| kinds.iter().next())
}

/// The body of one operation: whether it must be sent, its content type and its schema.
pub fn body_of<'a>(spec: &'a Value, operation: &'a Value) -> Option<(bool, String, &'a Value)> {
    if let Some(body) = operation.get("requestBody") {
        let body = resolve(spec, body);
        let (kind, content) = content(body)?;
        let required = body.get("required").and_then(Value::as_bool).unwrap_or(false);
        return Some((required, kind.clone(), content));
    }
    // Swagger 2 keeps the body among the parameters.
    let kind = || list(operation, "consumes").first().or_else(|| list(spec, "consumes").first()).map(said);
    list(operation, "parameters")
        .iter()
        .map(|one| resolve(spec, one))
        .find(|one| one.get("in") == Some(&json!("body")))
        .map(|one| {
            let required = one.get("required").and_then(Value::as_bool).unwrap_or(false);
            (required, kind().unwrap_or_else(|| "application/json".into()), one)
        })
}

/// The schema of what one answer holds, when the description says.
pub fn answer_schema<'a>(spec: &'a Value, answer: &'a Value) -> Option<&'a Value> {
    let answer = resolve(spec, answer);
    match content(answer) {
        Some((_, content)) => content.get("schema"),
        None => answer.get("schema"),
    }
}

fn pretty(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// A body to start from, the way its content type is written.
fn body_example(spec: &Value, kind: &str, content: &Value) -> String {
    let named = content.get("examples").and_then(Value::as_object).and_then(|all| all.values().next());
    let value =
        match content.get("example").or_else(|| named.map(|one| resolve(spec, one)).and_then(|one| one.get("value"))) {
            Some(given) => given.clone(),
            None => content.get("schema").map(|schema| sample(spec, schema)).unwrap_or(Value::Null),
        };
    match &value {
        Value::Object(fields) if kind.contains("x-www-form-urlencoded") => {
            fields.iter().map(|(name, value)| format!("{name}={}", said(value))).collect::<Vec<_>>().join("&")
        }
        // A string is JSON only with its quotes on.
        Value::String(_) if kind.contains("json") => value.to_string(),
        other => pretty(other),
    }
}

/// The form fields of Swagger 2, as a body.
fn form_of(spec: &Value, operation: &Value) -> Option<Body> {
    let fields: Vec<&Value> = list(operation, "parameters")
        .iter()
        .map(|one| resolve(spec, one))
        .filter(|one| one.get("in") == Some(&json!("formData")))
        .collect();
    if fields.is_empty() {
        return None;
    }
    let example = fields.iter().map(|one| format!("{}=", text(one, "name"))).collect::<Vec<_>>().join("&");
    let required = fields.iter().any(|one| one.get("required") == Some(&json!(true)));
    Some(Body { kind: "application/x-www-form-urlencoded".into(), required, example })
}

/// Every operation of a description, path by path.
pub fn operations(spec: &Value) -> Vec<Operation> {
    let mut found = Vec::new();
    let Some(paths) = spec.get("paths").and_then(Value::as_object) else { return found };
    for (path, item) in paths {
        let item = resolve(spec, item);
        for method in METHODS {
            let Some(operation) = item.get(method).filter(|one| one.is_object()) else { continue };
            let parameters = parameters_of(spec, item, operation)
                .into_iter()
                .map(|one| {
                    let schema = parameter_schema(one);
                    let example = one.get("example").cloned().unwrap_or_else(|| {
                        let schema = resolve(spec, schema);
                        // Only what the author wrote: a made-up value in a filter would be sent as if meant.
                        let given = ["example", "default"].iter().find_map(|name| schema.get(*name));
                        given.or_else(|| list(schema, "enum").first()).cloned().unwrap_or(Value::Null)
                    });
                    Parameter {
                        name: text(one, "name"),
                        place: text(one, "in"),
                        required: one.get("required").and_then(Value::as_bool).unwrap_or(false)
                            || text(one, "in") == "path",
                        kind: kind_of(spec, schema),
                        example: match example {
                            Value::Null => String::new(),
                            other => said(&other),
                        },
                        about: text(one, "description"),
                    }
                })
                .collect();
            let body = match body_of(spec, operation) {
                Some((required, kind, content)) => {
                    let example = body_example(spec, &kind, content);
                    Some(Body { kind, required, example })
                }
                None => form_of(spec, operation),
            };
            let summary = match text(operation, "summary") {
                empty if empty.is_empty() => {
                    text(operation, "description").lines().next().unwrap_or_default().to_string()
                }
                summary => summary,
            };
            found.push(Operation {
                id: format!("{} {path}", method.to_uppercase()),
                method: method.to_uppercase(),
                path: path.clone(),
                summary,
                tags: list(operation, "tags").iter().map(said).collect(),
                deprecated: operation.get("deprecated").and_then(Value::as_bool).unwrap_or(false),
                parameters,
                body,
                responses: operation
                    .get("responses")
                    .and_then(Value::as_object)
                    .map(|all| all.keys().cloned().collect())
                    .unwrap_or_default(),
                change: None,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shop() -> Value {
        json!({
            "openapi": "3.0.3",
            "info": { "title": "Shop", "version": "1.2" },
            "servers": [{ "url": "https://shop.example.com/api/v1/" }],
            "paths": {
                "/orders/{id}": {
                    "parameters": [{ "name": "id", "in": "path", "schema": { "type": "integer" } }],
                    "get": {
                        "summary": "One order",
                        "tags": ["orders"],
                        "parameters": [{ "$ref": "#/components/parameters/Expand" }],
                        "responses": { "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Order" } } } }, "404": {} }
                    },
                    "put": {
                        "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Order" } } } },
                        "responses": { "204": {} }
                    }
                }
            },
            "components": {
                "parameters": { "Expand": { "name": "expand", "in": "query", "schema": { "type": "string", "enum": ["lines", "buyer"] } } },
                "schemas": {
                    "Order": {
                        "type": "object",
                        "required": ["id", "lines"],
                        "properties": {
                            "id": { "type": "integer", "readOnly": true },
                            "state": { "type": "string", "enum": ["open", "paid"] },
                            "placed": { "type": "string", "format": "date-time", "nullable": true },
                            "lines": { "type": "array", "items": { "$ref": "#/components/schemas/Line" } },
                            "parent": { "$ref": "#/components/schemas/Order" }
                        }
                    },
                    "Line": { "type": "object", "required": ["sku"], "properties": { "sku": { "type": "string" }, "count": { "type": "integer", "minimum": 1 } } }
                }
            }
        })
    }

    #[test]
    fn a_description_is_read_from_json_and_from_yaml() {
        assert!(read(shop().to_string().as_bytes()).is_ok());
        let yaml =
            b"openapi: 3.1.0\ninfo:\n  title: Small\npaths:\n  /ping:\n    get:\n      responses:\n        '200': {}\n";
        let spec = read(yaml).unwrap();
        assert_eq!(operations(&spec)[0].id, "GET /ping");
        assert_eq!(operations(&spec)[0].responses, ["200"]);
    }

    #[test]
    fn what_is_not_a_description_is_refused_in_words() {
        assert!(read(b"<html>").unwrap_err().to_string().contains("neither JSON nor YAML"));
        assert!(read(b"{\"name\": \"x\"}").unwrap_err().to_string().contains("not an OpenAPI"));
    }

    #[test]
    fn the_operations_carry_the_parameters_of_their_path_and_their_own() {
        let found = operations(&shop());
        assert_eq!(
            found.iter().map(|one| one.id.as_str()).collect::<Vec<_>>(),
            ["GET /orders/{id}", "PUT /orders/{id}"]
        );
        let get = &found[0];
        assert_eq!(get.summary, "One order");
        assert_eq!(get.responses, ["200", "404"]);
        let names: Vec<_> =
            get.parameters.iter().map(|one| (one.name.as_str(), one.place.as_str(), one.required)).collect();
        assert_eq!(names, [("id", "path", true), ("expand", "query", false)]);
        assert_eq!(get.parameters[1].example, "lines", "the first of the listed values is a value to start from");
        assert!(get.body.is_none());
    }

    #[test]
    fn a_body_to_start_from_is_made_of_the_schema_without_what_only_the_service_writes() {
        let put = &operations(&shop())[1];
        let body = put.body.as_ref().unwrap();
        assert!(body.required);
        assert_eq!(body.kind, "application/json");
        let made: Value = serde_json::from_str(&body.example).unwrap();
        assert!(made.get("id").is_none(), "the id is the service's to write");
        assert_eq!(made["state"], "open");
        assert_eq!(made["placed"], "2026-01-01T00:00:00Z");
        assert_eq!(made["lines"][0], json!({ "sku": "string", "count": 1 }));
    }

    #[test]
    fn a_shape_names_every_place_and_ends_where_it_holds_itself() {
        let spec = shop();
        let made = shape(&spec, &json!({ "$ref": "#/components/schemas/Order" }));
        assert_eq!(made[""].kind, "object");
        assert!(made["id"].required && !made["state"].required);
        assert_eq!(made["state"].values, ["open", "paid"]);
        assert!(made["placed"].nullable);
        assert_eq!(made["lines"].kind, "object[]");
        assert!(made["lines[].sku"].required);
        assert!(made.contains_key("parent.parent.id"), "a shape inside itself is followed some way");
        assert!(made.len() < 200, "and not for ever");
    }

    #[test]
    fn the_prefix_is_the_path_of_the_first_server() {
        assert_eq!(prefix(&shop()), "/api/v1");
        assert_eq!(prefix(&json!({ "servers": [{ "url": "/v2" }] })), "/v2");
        assert_eq!(prefix(&json!({ "swagger": "2.0", "basePath": "/api" })), "/api");
        assert_eq!(prefix(&json!({ "servers": [{ "url": "https://example.com" }] })), "");
        assert_eq!(
            prefix(&json!({ "servers": [{ "url": "/{version}" }] })),
            "",
            "a path with a blank to fill is not guessed"
        );
    }

    #[test]
    fn swagger_two_keeps_the_body_among_the_parameters() {
        let spec = json!({
            "swagger": "2.0",
            "consumes": ["application/json"],
            "paths": { "/pets": { "post": {
                "parameters": [
                    { "name": "pet", "in": "body", "required": true, "schema": { "type": "object", "properties": { "name": { "type": "string", "example": "Rex" } } } },
                    { "name": "trace", "in": "header", "type": "string" }
                ],
                "responses": { "201": { "schema": { "type": "object" } } }
            } } }
        });
        let post = &operations(&spec)[0];
        assert_eq!(post.parameters.len(), 1);
        assert_eq!(post.parameters[0].kind, "string");
        let body = post.body.as_ref().unwrap();
        assert!(body.required);
        assert_eq!(serde_json::from_str::<Value>(&body.example).unwrap(), json!({ "name": "Rex" }));
    }
}
