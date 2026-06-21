use super::*;
use vercel_ai_provider::SimpleProvider;

#[test]
fn test_set_and_get_provider() {
    clear_default_provider();
    assert!(!has_default_provider());
    assert!(get_default_provider().is_none());

    let provider = Arc::new(SimpleProvider::new("test"));
    set_default_provider(provider);

    assert!(has_default_provider());
    let retrieved = get_default_provider();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().provider(), "test");

    clear_default_provider();
    assert!(!has_default_provider());
}
