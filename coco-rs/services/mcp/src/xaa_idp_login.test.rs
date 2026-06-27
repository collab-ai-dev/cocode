use super::IdpMetadata;
use super::ensure_token_exchange_supported;

fn metadata_with_grants(grants: Vec<&str>) -> IdpMetadata {
    IdpMetadata {
        token_endpoint: "https://idp.example/token".into(),
        issuer: "https://idp.example".into(),
        jwks_uri: None,
        grant_types: grants.into_iter().map(str::to_string).collect(),
    }
}

#[test]
fn empty_grants_list_is_lenient() {
    let m = metadata_with_grants(vec![]);
    assert!(ensure_token_exchange_supported(&m).is_ok());
}

#[test]
fn token_exchange_grant_accepted() {
    let m = metadata_with_grants(vec![
        "authorization_code",
        super::super::xaa::TOKEN_EXCHANGE_GRANT,
    ]);
    assert!(ensure_token_exchange_supported(&m).is_ok());
}

#[test]
fn missing_token_exchange_rejected() {
    let m = metadata_with_grants(vec!["authorization_code", "refresh_token"]);
    assert!(ensure_token_exchange_supported(&m).is_err());
}
