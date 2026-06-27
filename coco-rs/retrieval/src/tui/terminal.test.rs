// Terminal tests are difficult to run in CI, so we just verify compilation
#[test]
fn test_types_exist() {
    // Just ensure the types compile
    fn _check_types() {
        let _: fn() -> std::io::Result<super::Tui> = super::init;
        let _: fn() -> std::io::Result<()> = super::restore;
    }
}
