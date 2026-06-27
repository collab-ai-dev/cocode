use super::*;
use serde_json::json;

#[test]
fn test_value_of_string() {
    let value = json!("hello");
    assert_eq!(value_of::<String>(&value), Some("hello".to_string()));
}

#[test]
fn test_value_of_i64() {
    let value = json!(42);
    assert_eq!(value_of::<i64>(&value), Some(42));
}

#[test]
fn test_value_of_bool() {
    let value = json!(true);
    assert_eq!(value_of::<bool>(&value), Some(true));
}

#[test]
fn test_get_nested_value() {
    let value = json!({
        "a": {
            "b": {
                "c": 42
            }
        }
    });

    let nested = get_nested_value(&value, "a.b.c");
    assert_eq!(nested, Some(&json!(42)));
}

#[test]
fn test_get_nested() {
    let value = json!({
        "user": {
            "name": "Alice",
            "age": 30
        }
    });

    assert_eq!(
        get_nested::<String>(&value, "user.name"),
        Some("Alice".to_string())
    );
    assert_eq!(get_nested::<i64>(&value, "user.age"), Some(30));
}

#[test]
fn test_get_nested_array() {
    let value = json!({
        "items": [1, 2, 3]
    });

    assert_eq!(get_nested::<i64>(&value, "items.0"), Some(1));
    assert_eq!(get_nested::<i64>(&value, "items.2"), Some(3));
}