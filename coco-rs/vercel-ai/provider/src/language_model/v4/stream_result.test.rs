use super::*;
use futures::stream;

#[test]
fn test_stream_result_debug() {
    let stream = stream::empty();
    let result = LanguageModelV4StreamResult::new(Box::pin(stream));
    let debug_str = format!("{result:?}");
    assert!(debug_str.contains("LanguageModelV4StreamResult"));
}
