use super::*;
use serde_json::json;

fn arg(json_path: &str) -> PartialArg {
    PartialArg {
        json_path: json_path.into(),
        ..Default::default()
    }
}

#[test]
fn single_string_value() {
    let mut acc = GoogleJSONAccumulator::new();
    let r = acc.process_partial_args(&[PartialArg {
        json_path: "$.location".into(),
        string_value: Some("Boston".into()),
        ..Default::default()
    }]);
    assert_eq!(r.text_delta, r#"{"location":"Boston""#);
    assert_eq!(r.current_json, json!({ "location": "Boston" }));

    let f = acc.finalize();
    assert_eq!(f.closing_delta, "}");
    assert_eq!(f.final_json, r#"{"location":"Boston"}"#);
}

#[test]
fn single_number_value() {
    let mut acc = GoogleJSONAccumulator::new();
    let r = acc.process_partial_args(&[PartialArg {
        json_path: "$.brightness".into(),
        number_value: Some(50.0),
        ..Default::default()
    }]);
    assert_eq!(r.text_delta, r#"{"brightness":50"#);
    assert_eq!(r.current_json, json!({ "brightness": 50 }));
}

#[test]
fn boolean_and_null_values() {
    let mut acc = GoogleJSONAccumulator::new();
    let r = acc.process_partial_args(&[
        PartialArg {
            json_path: "$.on".into(),
            bool_value: Some(true),
            ..Default::default()
        },
        PartialArg {
            json_path: "$.off".into(),
            null_value: Some(Value::Null),
            ..Default::default()
        },
    ]);
    assert_eq!(r.text_delta, r#"{"on":true,"off":null"#);
    let f = acc.finalize();
    assert_eq!(f.final_json, r#"{"on":true,"off":null}"#);
}

#[test]
fn nested_object_path() {
    let mut acc = GoogleJSONAccumulator::new();
    let r = acc.process_partial_args(&[PartialArg {
        json_path: "$.recipe.name".into(),
        string_value: Some("Lasagna".into()),
        ..Default::default()
    }]);
    assert_eq!(r.text_delta, r#"{"recipe":{"name":"Lasagna""#);
    let f = acc.finalize();
    assert_eq!(f.final_json, r#"{"recipe":{"name":"Lasagna"}}"#);
}

#[test]
fn array_index_path() {
    let mut acc = GoogleJSONAccumulator::new();
    let r = acc.process_partial_args(&[PartialArg {
        json_path: "$.tags[0]".into(),
        string_value: Some("fast".into()),
        ..Default::default()
    }]);
    // Open root, open "tags":[, leaf "fast"
    assert_eq!(r.text_delta, r#"{"tags":["fast""#);
    let f = acc.finalize();
    assert_eq!(f.final_json, r#"{"tags":["fast"]}"#);
}

#[test]
fn split_string_continuation() {
    let mut acc = GoogleJSONAccumulator::new();
    let r1 = acc.process_partial_args(&[PartialArg {
        json_path: "$.name".into(),
        string_value: Some("Hello".into()),
        will_continue: Some(true),
        ..Default::default()
    }]);
    // First chunk: open without closing quote
    assert_eq!(r1.text_delta, r#"{"name":"Hello"#);
    let r2 = acc.process_partial_args(&[PartialArg {
        json_path: "$.name".into(),
        string_value: Some(" World".into()),
        ..Default::default()
    }]);
    // Second chunk: appended escaped without reopening; will be closed at finalize
    assert_eq!(r2.text_delta, " World");
    let f = acc.finalize();
    assert_eq!(f.final_json, r#"{"name":"Hello World"}"#);
}

#[test]
fn path_parser_handles_brackets() {
    use super::parse_path;
    let segs = parse_path("recipe.ingredients[0].name");
    assert_eq!(segs.len(), 4);
    matches!(&segs[0], PathSegment::Key(k) if k == "recipe");
    matches!(&segs[1], PathSegment::Key(k) if k == "ingredients");
    matches!(&segs[2], PathSegment::Index(0));
    matches!(&segs[3], PathSegment::Key(k) if k == "name");
}

#[test]
fn ignores_empty_or_root_path() {
    let mut acc = GoogleJSONAccumulator::new();
    let r = acc.process_partial_args(&[
        arg("$"),
        arg("$."),
        PartialArg {
            json_path: "$.x".into(),
            number_value: Some(1.0),
            ..Default::default()
        },
    ]);
    assert_eq!(r.current_json, json!({ "x": 1 }));
}
