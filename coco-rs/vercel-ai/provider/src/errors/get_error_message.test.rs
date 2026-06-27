use super::*;

#[test]
fn test_get_error_message() {
    let err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    assert_eq!(get_error_message(&err), "file not found");
}
