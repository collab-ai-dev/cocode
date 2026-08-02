use super::*;
use serde_json::json;
use std::collections::HashSet;

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

#[test]
fn projection_rejects_a_wire_alias_not_created_by_the_current_transaction() {
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
    let mut input = json!({(wire_name.clone()): "caller value"});

    assert!(matches!(
        aliases.project_input("user", &mut input),
        Err(ToolSchemaAliasError::UnexpectedPropertyForm {
            encountered_name,
            ..
        }) if encountered_name == wire_name
    ));
    assert_eq!(input, json!({(wire_name): "caller value"}));
}

#[test]
fn alternatives_share_one_injective_alias_namespace() {
    let schema = json!({
        "oneOf": [
            {"type": "object", "properties": {"display name": {"type": "string"}}},
            {"type": "object", "properties": {"display/name": {"type": "string"}}},
            {"type": "object", "properties": {"display_name": {"type": "string"}}}
        ]
    });
    let (projected, projection) = project_schema(&schema);
    let names: Vec<String> = projected["oneOf"]
        .as_array()
        .expect("alternatives")
        .iter()
        .map(|branch| {
            branch["properties"]
                .as_object()
                .expect("properties")
                .keys()
                .next()
                .expect("property")
                .clone()
        })
        .collect();
    assert_eq!(names.iter().collect::<HashSet<_>>().len(), 3);

    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("choice".into(), projection);
    let mut input = json!({
        names[0].clone(): "Ada",
        names[1].clone(): "Grace",
        names[2].clone(): "valid"
    });
    aliases
        .restore_input("choice", &mut input)
        .expect("restore alternatives");
    assert_eq!(input["display name"], "Ada");
    assert_eq!(input["display/name"], "Grace");
    assert_eq!(input["display_name"], "valid");
}

#[test]
fn restores_arbitrary_local_refs_and_recursive_values() {
    let schema = json!({
        "type": "object",
        "properties": {
            "node": {
                "type": "object",
                "properties": {
                    "display name": {"type": "string"},
                    "child": {"$ref": "#/properties/node"}
                }
            }
        }
    });
    let (projected, projection) = project_schema(&schema);
    let wire_name = projected["properties"]["node"]["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .find(|name| name.as_str() != "child")
        .expect("wire name")
        .clone();
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("tree".into(), projection);
    let mut input = json!({
        "node": {
            wire_name.clone(): "root",
            "child": {wire_name: "leaf"}
        }
    });
    aliases
        .restore_input("tree", &mut input)
        .expect("restore recursive aliases");
    assert_eq!(input["node"]["display name"], "root");
    assert_eq!(input["node"]["child"]["display name"], "leaf");
}

#[test]
fn rewrites_refs_that_point_through_aliased_properties() {
    let schema = json!({
        "type": "object",
        "properties": {"entry/value": {"type": "object", "properties": {"inner key": {}}}},
        "additionalProperties": {"$ref": "#/properties/entry~1value"}
    });
    let (projected, _) = project_schema(&schema);
    let wire_name = projected["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .next()
        .expect("wire name");
    assert_eq!(
        projected["additionalProperties"]["$ref"],
        format!("#/properties/{}", escape_json_pointer(wire_name))
    );
}

#[test]
fn traverses_schema_bearing_object_and_array_keywords() {
    let nested = || json!({"type": "object", "properties": {"inner key": {}}});
    let schema = json!({
        "type": "object",
        "properties": {"trigger key": {}},
        "patternProperties": {"^pattern": nested()},
        "additionalProperties": nested(),
        "dependentSchemas": {"trigger key": {"properties": {"dependent key": {}}}},
        "allOf": [{"properties": {"all key": {}}}],
        "if": {"properties": {"if key": {}}},
        "then": {"properties": {"then key": {}}},
        "else": {"properties": {"else key": {}}},
        "properties": {
            "trigger key": {},
            "array": {
                "type": "array",
                "contains": nested(),
                "items": nested()
            }
        }
    });
    let (projected, projection) = project_schema(&schema);
    let alias_for = |original: &str| {
        projection
            .nodes_by_pointer
            .values()
            .flat_map(|node| &node.properties)
            .find(|property| property.original_name == original)
            .map(|property| property.wire_name.clone())
            .expect("projected alias")
    };
    assert!(projected.is_object());
    let trigger = alias_for("trigger key");
    let inner = alias_for("inner key");
    let dependent = alias_for("dependent key");
    let all = alias_for("all key");
    let if_key = alias_for("if key");
    let then_key = alias_for("then key");
    let else_key = alias_for("else key");
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("keywords".into(), projection);
    let mut input = json!({
        (trigger): true,
        (dependent): 1,
        (all): 2,
        (if_key): 3,
        (then_key): 4,
        (else_key): 5,
        "pattern-one": {(inner.clone()): "pattern"},
        "extra": {(inner.clone()): "additional"},
        "array": [{(inner): "item"}]
    });
    aliases
        .restore_input("keywords", &mut input)
        .expect("restore keyword aliases");
    assert!(input.get("trigger key").is_some());
    assert!(input.get("dependent key").is_some());
    assert!(input.get("all key").is_some());
    assert!(input.get("if key").is_some());
    assert!(input.get("then key").is_some());
    assert!(input.get("else key").is_some());
    assert_eq!(input["pattern-one"]["inner key"], "pattern");
    assert_eq!(input["extra"]["inner key"], "additional");
    assert_eq!(input["array"][0]["inner key"], "item");
}

#[test]
fn pattern_properties_continue_to_constrain_projected_property_names() {
    let schema = json!({
        "type": "object",
        "properties": {"bad/name": {"type": "object"}},
        "patternProperties": {
            "^bad/name$": {
                "type": "object",
                "properties": {"nested key": {"type": "string"}}
            }
        }
    });
    let (projected, projection) = project_schema(&schema);
    let wire_name = projected["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .next()
        .expect("wire property")
        .clone();
    let patterns = projected["patternProperties"]
        .as_object()
        .expect("pattern properties");
    let (_, supplemental_schema) = patterns
        .iter()
        .find(|(pattern, _)| {
            pattern.as_str() != "^bad/name$"
                && regex::Regex::new(pattern)
                    .expect("projected pattern")
                    .is_match(&wire_name)
        })
        .expect("supplemental alias pattern");
    let nested_wire = supplemental_schema["properties"]
        .as_object()
        .expect("nested properties")
        .keys()
        .next()
        .expect("nested wire property")
        .clone();

    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("patterned".into(), projection);
    let mut input = json!({(wire_name): {(nested_wire): "value"}});
    aliases
        .restore_input("patterned", &mut input)
        .expect("restore patterned aliases");
    assert_eq!(input, json!({"bad/name": {"nested key": "value"}}));
}

#[test]
fn nested_collision_leaves_entire_input_unchanged() {
    let schema = json!({
        "type": "object",
        "properties": {
            "outer key": {
                "type": "object",
                "properties": {"inner key": {}}
            }
        }
    });
    let (_, projection) = project_schema(&schema);
    let outer = projection
        .root
        .properties
        .iter()
        .find(|property| property.original_name == "outer key")
        .expect("outer alias");
    let inner = outer
        .child
        .properties
        .iter()
        .find(|property| property.original_name == "inner key")
        .expect("inner alias");
    let mut input = json!({
        outer.wire_name.clone(): {
            inner.wire_name.clone(): "wire",
            "inner key": "original"
        }
    });
    let original = input.clone();
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("atomic".into(), projection);
    assert!(aliases.restore_input("atomic", &mut input).is_err());
    assert_eq!(input, original);
}

#[test]
fn all_of_reapplies_child_schemas_after_parent_was_renamed() {
    let schema = json!({
        "allOf": [
            {
                "properties": {
                    "payload value": {
                        "properties": {"first child": {}}
                    }
                }
            },
            {
                "properties": {
                    "payload value": {
                        "properties": {"second child": {}}
                    }
                }
            }
        ]
    });
    let (_, projection) = project_schema(&schema);
    let alias_for = |original: &str| {
        projection
            .nodes_by_pointer
            .values()
            .flat_map(|node| &node.properties)
            .find(|property| property.original_name == original)
            .map(|property| property.wire_name.clone())
            .expect("alias")
    };
    let parent = alias_for("payload value");
    let first = alias_for("first child");
    let second = alias_for("second child");
    let mut input = json!({(parent): {(first): 1, (second): 2}});
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("all-of".into(), projection);
    aliases
        .restore_input("all-of", &mut input)
        .expect("restore every applicable child schema");
    assert_eq!(input["payload value"]["first child"], 1);
    assert_eq!(input["payload value"]["second child"], 2);
}

#[test]
fn traverses_legacy_schema_dependencies() {
    let schema = json!({
        "type": "object",
        "properties": {"trigger key": {}},
        "dependencies": {
            "trigger key": {
                "properties": {"dependent value": {}}
            }
        }
    });
    let (projected, projection) = project_schema(&schema);
    let projected_dependencies = projected["dependencies"].as_object().expect("dependencies");
    let trigger = projected_dependencies
        .keys()
        .next()
        .expect("trigger")
        .clone();
    let dependent = projected_dependencies[&trigger]["properties"]
        .as_object()
        .expect("dependent properties")
        .keys()
        .next()
        .expect("dependent")
        .clone();
    let mut input = json!({(trigger): true, (dependent): "restored"});
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("legacy".into(), projection);
    aliases
        .restore_input("legacy", &mut input)
        .expect("restore legacy dependency schema");
    assert_eq!(input["dependent value"], "restored");
}

#[test]
fn required_only_property_names_are_projected_and_restored() {
    let schema = json!({"type": "object", "required": ["required value"]});
    let (projected, projection) = project_schema(&schema);
    let wire_name = projected["required"][0]
        .as_str()
        .expect("projected required name")
        .to_string();
    assert_ne!(wire_name, "required value");
    let mut input = json!({(wire_name): 42});
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("required-only".into(), projection);
    aliases
        .restore_input("required-only", &mut input)
        .expect("restore required-only name");
    assert_eq!(input["required value"], 42);
}

#[test]
fn restores_legacy_tuple_items_and_additional_items() {
    let schema = json!({
        "type": "array",
        "items": [
            {"type": "object", "properties": {"first value": {}}}
        ],
        "additionalItems": {
            "type": "object",
            "properties": {"rest value": {}}
        }
    });
    let (_, projection) = project_schema(&schema);
    let alias_for = |original: &str| {
        projection
            .nodes_by_pointer
            .values()
            .flat_map(|node| &node.properties)
            .find(|property| property.original_name == original)
            .map(|property| property.wire_name.clone())
            .expect("alias")
    };
    let first = alias_for("first value");
    let rest = alias_for("rest value");
    let mut input = json!([{(first): 1}, {(rest): 2}]);
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("tuple".into(), projection);
    aliases
        .restore_input("tuple", &mut input)
        .expect("restore tuple aliases");
    assert_eq!(input[0]["first value"], 1);
    assert_eq!(input[1]["rest value"], 2);
}

#[test]
fn negated_schema_uses_the_same_key_projection() {
    let schema = json!({
        "type": "object",
        "not": {
            "required": ["forbidden value"]
        }
    });
    let (projected, projection) = project_schema(&schema);
    let wire_name = projected["not"]["required"][0]
        .as_str()
        .expect("projected negated key")
        .to_string();
    let mut input = json!({(wire_name): true});
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("not".into(), projection);
    aliases
        .restore_input("not", &mut input)
        .expect("restore negated-schema key");
    assert_eq!(input["forbidden value"], true);
}

#[test]
fn restoration_accepts_input_the_model_wrote_with_the_original_name() {
    // Aliases are opaque digests, so a model will sometimes emit the
    // human-readable key it saw in the tool description instead. In the
    // from-wire direction that key is already correct — rejecting it would
    // cost a `<tool_use_error>` round trip for input that needs no repair.
    let schema = json!({
        "type": "object",
        "properties": {"display name": {"type": "string"}}
    });
    let (_, projection) = project_schema(&schema);
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("user".into(), projection);

    let mut input = json!({"display name": "Ada"});
    aliases.restore_input("user", &mut input).expect("restored");
    assert_eq!(input, json!({"display name": "Ada"}));
}

#[test]
fn a_schema_without_invalid_names_allocates_no_projection() {
    let schema = json!({
        "type": "object",
        "properties": {
            "file_path": {"type": "string"},
            "limit": {"type": "integer"},
            "nested": {
                "type": "object",
                "properties": {"offset": {"type": "integer"}}
            }
        },
        "required": ["file_path"]
    });
    let (projected, projection) = project_schema(&schema);

    assert_eq!(projected, schema, "a clean schema goes out untouched");
    let mut aliases = ToolSchemaAliases::default();
    aliases.insert("Read".into(), projection);
    assert!(!aliases.requires_projection("Read"));
    assert!(aliases.is_empty());

    // And the transform seams are no-ops rather than schema walks.
    let mut input = json!({"file_path": "/tmp/x", "unknown": {"deep": 1}});
    let before = input.clone();
    aliases
        .project_input("Read", &mut input)
        .expect("projected");
    aliases.restore_input("Read", &mut input).expect("restored");
    assert_eq!(input, before);
}

#[test]
fn unprojectable_history_input_is_kept_verbatim_instead_of_failing_the_request() {
    // The same history is replayed on every turn: a hard error here would
    // brick the session until compaction dropped the offending message.
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

    let poisoned = json!({(wire_name.clone()): "already projected"});
    let mut messages = vec![json!({
        "role": "assistant",
        "content": [
            {"type": "tool_use", "id": "toolu_1", "name": "user", "input": poisoned},
            {"type": "tool_use", "id": "toolu_2", "name": "user",
             "input": {"display name": "Grace"}},
        ]
    })];

    let skipped = aliases.project_message_tool_inputs(&mut messages);
    assert_eq!(skipped, vec!["user".to_string()]);

    let blocks = messages[0]["content"].as_array().expect("content");
    assert_eq!(
        blocks[0]["input"],
        json!({(wire_name.clone()): "already projected"}),
        "the unprojectable block is sent exactly as it arrived"
    );
    assert_eq!(
        blocks[1]["input"],
        json!({(wire_name): "Grace"}),
        "a sibling block still projects normally"
    );
}
