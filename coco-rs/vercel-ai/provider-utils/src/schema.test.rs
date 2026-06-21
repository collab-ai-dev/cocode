//! Tests for schema utilities.

use super::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, PartialEq)]
struct Person {
    name: String,
    age: u32,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Product {
    id: String,
    price: f64,
    #[serde(default)]
    in_stock: bool,
}

fn object_schema() -> JSONSchema {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer", "minimum": 0 }
        },
        "required": ["name", "age"]
    })
}

#[test]
fn test_json_schema_valid_input() {
    let schema: JsonSchemaWrapper<Person> = JsonSchemaWrapper::new(object_schema());
    let value = json!({ "name": "Alice", "age": 30 });

    let result = schema.validate(&value);
    assert!(result.is_ok());
    let person = result.unwrap();
    assert_eq!(person.name, "Alice");
    assert_eq!(person.age, 30);
}

#[test]
fn test_json_schema_missing_required_field() {
    let schema: JsonSchemaWrapper<Person> = JsonSchemaWrapper::new(object_schema());
    let value = json!({ "name": "Alice" }); // missing age

    let result = schema.validate(&value);
    assert!(result.is_err());
    match result.unwrap_err() {
        ValidationError::ParseError(msg) => {
            assert!(msg.contains("missing field"));
        }
        ValidationError::SchemaValidation(errors) => {
            // With schema validation, we get validation errors
            assert!(!errors.is_empty());
        }
    }
}

#[test]
fn test_json_schema_wrong_type() {
    let schema: JsonSchemaWrapper<Person> = JsonSchemaWrapper::new(object_schema());
    let value = json!({ "name": "Bob", "age": "thirty" }); // age should be integer

    let result = schema.validate(&value);
    assert!(result.is_err());
}

#[test]
fn test_json_schema_nested_object() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct Order {
        customer: Person,
        total: f64,
    }

    let schema: JsonSchemaWrapper<Order> = JsonSchemaWrapper::new(json!({
        "type": "object",
        "properties": {
            "customer": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" }
                },
                "required": ["name", "age"]
            },
            "total": { "type": "number" }
        },
        "required": ["customer", "total"]
    }));

    let value = json!({
        "customer": { "name": "Charlie", "age": 25 },
        "total": 99.99
    });

    let result = schema.validate(&value);
    assert!(result.is_ok());
    let order = result.unwrap();
    assert_eq!(order.customer.name, "Charlie");
    assert_eq!(order.total, 99.99);
}

#[test]
fn test_as_schema_valid_input() {
    let schema = as_schema::<Product>();
    let value = json!({ "id": "prod-123", "price": 29.99, "in_stock": true });

    let result = schema.validate(&value);
    assert!(result.is_ok());
    let product = result.unwrap();
    assert_eq!(product.id, "prod-123");
    assert_eq!(product.price, 29.99);
    assert!(product.in_stock);
}

#[test]
fn test_as_schema_with_defaults() {
    let schema = as_schema::<Product>();
    let value = json!({ "id": "prod-456", "price": 49.99 }); // in_stock omitted

    let result = schema.validate(&value);
    assert!(result.is_ok());
    let product = result.unwrap();
    assert!(!product.in_stock); // default false
}

#[test]
fn test_as_schema_not_object() {
    let schema = as_schema::<Person>();
    let value = json!("not an object");

    let result = schema.validate(&value);
    assert!(result.is_err());
}

#[test]
fn test_json_schema_array() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct Team {
        members: Vec<String>,
    }

    let schema: JsonSchemaWrapper<Team> = JsonSchemaWrapper::new(json!({
        "type": "object",
        "properties": {
            "members": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["members"]
    }));

    let value = json!({ "members": ["Alice", "Bob", "Charlie"] });
    let result = schema.validate(&value);
    assert!(result.is_ok());
    let team = result.unwrap();
    assert_eq!(team.members, vec!["Alice", "Bob", "Charlie"]);
}

#[test]
fn test_validation_error_creation() {
    let error = ValidationError::validation("test error");
    match error {
        ValidationError::SchemaValidation(errors) => {
            assert_eq!(errors, vec!["test error"]);
        }
        _ => panic!("Expected SchemaValidation variant"),
    }
}

#[cfg(feature = "schema-validation")]
mod schema_validation_tests {
    use super::*;

    #[test]
    fn test_schema_validation_minimum_constraint() {
        let schema: JsonSchemaWrapper<Person> = JsonSchemaWrapper::new(object_schema());
        // Age is negative, which violates minimum: 0
        let value = json!({ "name": "Alice", "age": -5 });

        let result = schema.validate(&value);
        // With schema validation enabled, this should fail validation
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_validation_additional_properties() {
        let schema: JSONSchema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"],
            "additionalProperties": false
        });

        let wrapper: JsonSchemaWrapper<serde_json::Value> = JsonSchemaWrapper::new(schema);

        // Additional property should cause validation error
        let value = json!({ "name": "Test", "extra": "field" });
        let result = wrapper.validate(&value);
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_validation_enum() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct StatusUpdate {
            status: String,
        }

        let schema: JsonSchemaWrapper<StatusUpdate> = JsonSchemaWrapper::new(json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["pending", "approved", "rejected"]
                }
            },
            "required": ["status"]
        }));

        // Valid enum value
        let valid = json!({ "status": "approved" });
        assert!(schema.validate(&valid).is_ok());

        // Invalid enum value
        let invalid = json!({ "status": "unknown" });
        assert!(schema.validate(&invalid).is_err());
    }

    #[test]
    fn test_schema_validation_pattern() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct User {
            email: String,
        }

        let schema: JsonSchemaWrapper<User> = JsonSchemaWrapper::new(json!({
            "type": "object",
            "properties": {
                "email": {
                    "type": "string",
                    "format": "email"
                }
            },
            "required": ["email"]
        }));

        // Note: format validation depends on jsonschema crate configuration
        let value = json!({ "email": "test@example.com" });
        assert!(schema.validate(&value).is_ok());
    }
}

#[test]
fn test_json_schema_trait() {
    // Test that Schema trait object works
    let schema: Box<dyn Schema<Output = Person>> =
        Box::new(JsonSchemaWrapper::<Person>::new(object_schema()));

    let value = json!({ "name": "Test", "age": 1 });
    let result = schema.validate(&value);
    assert!(result.is_ok());
}

#[test]
fn test_json_schema_get_schema() {
    let schema_value = object_schema();
    let wrapper: JsonSchemaWrapper<Person> = JsonSchemaWrapper::new(schema_value.clone());

    let retrieved = wrapper.json_schema();
    assert_eq!(retrieved, &schema_value);
}
