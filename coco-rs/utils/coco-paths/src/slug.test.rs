use super::*;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn slug_for_canonical_unix_path() {
    let slug = ProjectSlug::for_path(Path::new("/Users/foo/project"));
    assert_eq!(slug.as_str(), "-Users-foo-project");
}

#[test]
fn slug_eq_hash_friendly() {
    use std::collections::HashSet;
    let a = ProjectSlug::for_path(Path::new("/a/b"));
    let b = ProjectSlug::for_path(Path::new("/a/b"));
    let mut set: HashSet<ProjectSlug> = HashSet::new();
    set.insert(a);
    assert!(set.contains(&b));
}

#[test]
fn slug_nfc_folds_decomposed_input() {
    // NFC folds `e + combining-acute` to `é` (U+00E9). `é` is
    // outside ASCII alnum so sanitize maps it to `-`. The point of
    // this test is that decomposed and precomposed inputs produce
    // the SAME slug — without NFC they'd diverge into two different
    // directories.
    let decomposed = ProjectSlug::for_path(Path::new("/caf\u{0065}\u{0301}"));
    let precomposed = ProjectSlug::for_path(Path::new("/caf\u{00E9}"));
    assert_eq!(decomposed, precomposed);
}

#[test]
fn slug_display_and_as_ref() {
    let slug = ProjectSlug::for_path(Path::new("/x/y"));
    assert_eq!(format!("{slug}"), "-x-y");
    let s: &str = slug.as_ref();
    assert_eq!(s, "-x-y");
}
