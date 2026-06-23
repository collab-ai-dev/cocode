use rquickjs::Context;
use rquickjs::Runtime;

#[test]
fn json_value_round_trips_through_js() {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let value = serde_json::json!({
            "name": "release",
            "count": 3,
            "ratio": 2.5,
            "flags": [true, false, null],
            "nested": { "x": "y" }
        });
        let js = super::json_to_js(&ctx, &value).expect("to js");
        let back = super::js_to_json(&ctx, js).expect("to json");
        assert_eq!(back, value);
    });
}

#[test]
fn undefined_and_functions_become_null() {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let js = ctx.eval::<rquickjs::Value, _>("undefined").expect("eval");
        assert_eq!(
            super::js_to_json(&ctx, js).expect("json"),
            serde_json::Value::Null
        );
    });
}
