//! `providers` block — the built-in provider catalog.
//!
//! Every cell comes from `coco_config::builtin_providers()`; there is nothing
//! hand-maintained here. Rows follow the catalog's vendor-grouped order, which
//! `common/config/src/builtin/mod.rs` documents as byte-stable.

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use coco_config::ProviderAuth;
use coco_config::builtin_providers;
use coco_provider_auth::descriptor_for;

pub fn render() -> Result<String> {
    let providers = builtin_providers().context("resolving the builtin provider catalog")?;

    let mut out = String::from("| Provider | `api` | Auth | Base URL |\n| --- | --- | --- | --- |");
    for provider in providers {
        let auth = match provider.auth {
            ProviderAuth::ApiKey => format!("`{}`", provider.env_key),
            ProviderAuth::OAuth { flow } => {
                let Some(descriptor) = descriptor_for(flow) else {
                    bail!(
                        "provider `{}` declares OAuth flow `{flow}`, which has no descriptor in \
                         coco-provider-auth — add one so the docs can name the flow",
                        provider.name
                    );
                };
                format!("OAuth ({})", descriptor.display_name)
            }
        };
        let name = provider.name;
        let api = provider.api.as_str();
        let base_url = provider.base_url;
        out.push_str(&format!("\n| `{name}` | `{api}` | {auth} | `{base_url}` |"));
    }
    Ok(out)
}
