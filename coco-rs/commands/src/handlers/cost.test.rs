use super::*;

#[tokio::test]
async fn cost_handler_emits_sentinel() {
    let out = handler(String::new()).await.unwrap();
    assert!(
        out.starts_with(COST_SENTINEL),
        "must lead with the sentinel: {out}"
    );
    assert!(parse_cost_sentinel(&out).is_some());
}

#[test]
fn parse_cost_sentinel_matches_only_its_prefix() {
    assert!(parse_cost_sentinel("__COCO_COST__\nstatus").is_some());
    assert!(parse_cost_sentinel("hello world").is_none());
    assert!(parse_cost_sentinel("__COCO_OTHER__ foo").is_none());
}
