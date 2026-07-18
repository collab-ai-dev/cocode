use super::*;
use pretty_assertions::assert_eq;

#[test]
fn root_policy_is_public_durable_and_parentless() {
    let policy = SessionRegistrationPolicy::root();
    assert_eq!(policy.topology, SessionTopology::Root);
    assert_eq!(policy.visibility, SessionVisibility::Public);
    assert_eq!(policy.egress, SessionEgress::DurableHub);
    assert_eq!(policy.parent(), None);
    assert!(!policy.is_internal());
    assert!(!policy.is_local_only());
}

#[test]
fn side_chat_child_is_internal_local_only_with_parent() {
    let parent = SessionId::generate();
    let policy = SessionRegistrationPolicy::side_chat_child(parent.clone());
    assert_eq!(policy.parent(), Some(&parent));
    assert!(policy.is_internal());
    assert!(policy.is_local_only());
}
