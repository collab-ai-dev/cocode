use super::*;
use serde_json::json;

#[test]
fn projects_and_restores_nested_invalid_property_names() {
    let schema = json!({
        "type": "object",
        "properties": {
            "valid": {"type": "string"},
            "Cloudflare API Token": {
                "type": "object",
                "properties": {
                    "nested/key": {"type": "string"}
                },
                "required": ["nested/key"]
            }
        },
        "required": ["Cloudflare API Token"]
    });
    let (projected, projection) = project_schema(&schema);
    let properties = projected["properties"].as_object().expect("properties");
    let wire_name = properties
        .keys()
        .find(|name| name.as_str() != "valid")
        .expect("projected key")
        .clone();
    assert!(is_valid_property_name(&wire_name));
    assert_eq!(projected["required"][0], wire_name);

    let nested_wire = properties[&wire_name]["properties"]
        .as_object()
        .expect("nested properties")
        .keys()
        .next()
        .expect("nested key")
        .clone();
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("cloudflare".into(), projection);
    let mut input = json!({(wire_name): {(nested_wire): "secret"}, "valid": "yes"});
    aliases
        .restore_input("cloudflare", &mut input)
        .expect("restore aliases");
    assert_eq!(input["Cloudflare API Token"]["nested/key"], "secret");
    assert_eq!(input["valid"], "yes");
}

#[test]
fn projection_is_deterministic_and_collision_safe() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a b": {"type": "string"},
            "a/b": {"type": "string"},
            "a_b": {"type": "string"}
        }
    });
    let (first, _) = project_schema(&schema);
    let (second, _) = project_schema(&schema);
    assert_eq!(first, second);
    let keys: Vec<&String> = first["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .collect();
    assert_eq!(keys.len(), 3);
    assert_eq!(keys.iter().collect::<HashSet<_>>().len(), 3);
    assert!(keys.iter().all(|key| is_valid_property_name(key)));
}

#[test]
fn restores_array_items_and_local_refs() {
    let schema = json!({
        "$defs": {
            "entry": {
                "type": "object",
                "properties": {"display name": {"type": "string"}}
            }
        },
        "type": "array",
        "items": {"$ref": "#/$defs/entry"}
    });
    let (projected, projection) = project_schema(&schema);
    let wire_name = projected["$defs"]["entry"]["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .next()
        .expect("wire key")
        .clone();
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("list".into(), projection);
    let mut input = json!([{(wire_name): "Ada"}]);
    aliases
        .restore_input("list", &mut input)
        .expect("restore aliases");
    assert_eq!(input[0]["display name"], "Ada");
}

#[test]
fn restore_rejects_alias_and_original_collision() {
    let schema = json!({
        "type": "object",
        "properties": {"display name": {"type": "string"}}
    });
    let (projected, projection) = project_schema(&schema);
    let wire_name = projected["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .next()
        .expect("wire key")
        .clone();
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("user".into(), projection);
    let mut input = json!({(wire_name): "Ada", "display name": "Grace"});
    assert!(aliases.restore_input("user", &mut input).is_err());
}
