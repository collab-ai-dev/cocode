use rquickjs::Context;
use rquickjs::Runtime;

#[test]
fn quickjs_engine_builds_and_evaluates() {
    assert_eq!(super::eval_smoke().expect("eval"), 2);
}

#[test]
fn determinism_shim_forbids_nondeterministic_surfaces_at_runtime() {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        super::install_sandbox(&ctx).expect("install sandbox");

        // Nondeterministic surfaces throw at runtime.
        assert!(ctx.eval::<f64, _>("Math.random()").is_err());
        assert!(ctx.eval::<f64, _>("Date.now()").is_err());
        assert!(ctx.eval::<rquickjs::Value, _>("new Date()").is_err());
        assert!(ctx.eval::<rquickjs::Value, _>("Date()").is_err());

        // Explicit, deterministic date construction is still allowed.
        assert!(
            ctx.eval::<rquickjs::Value, _>("new Date(2020, 0, 1)")
                .is_ok()
        );

        // Deterministic arithmetic is unaffected.
        assert_eq!(ctx.eval::<i32, _>("2 + 3").expect("arith"), 5);
    });
}
