use super::is_preapproved_webfetch_url;

#[test]
fn exact_host_matches() {
    assert!(is_preapproved_webfetch_url(
        "https://docs.python.org/3/library/os.html"
    ));
}

#[test]
fn exact_host_rejects_subdomains() {
    assert!(!is_preapproved_webfetch_url("https://sub.docs.python.org/"));
}

#[test]
fn path_scoped_entry_requires_segment_boundary() {
    assert!(is_preapproved_webfetch_url(
        "https://github.com/anthropics/claude-code"
    ));
    assert!(!is_preapproved_webfetch_url(
        "https://github.com/anthropics-evil"
    ));
}
