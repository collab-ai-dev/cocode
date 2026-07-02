use pretty_assertions::assert_eq;

use super::filtered;
use crate::state::LoginEntry;
use crate::state::LoginPickerState;

fn entry(provider: &str, display: &str, auth: &str) -> LoginEntry {
    LoginEntry {
        provider: provider.to_string(),
        provider_display: display.to_string(),
        auth_label: auth.to_string(),
        logged_in: false,
    }
}

fn state(entries: Vec<LoginEntry>, filter: &str) -> LoginPickerState {
    LoginPickerState {
        entries,
        filter: filter.to_string(),
        selected: 0,
    }
}

#[test]
fn test_filtered_empty_filter_returns_all() {
    let l = state(
        vec![
            entry("openai-chatgpt", "openai-chatgpt", "OAuth"),
            entry("gemini-code-assist", "gemini-code-assist", "OAuth"),
        ],
        "",
    );
    assert_eq!(filtered(&l).len(), 2);
}

#[test]
fn test_filtered_matches_provider_display_substring() {
    let l = state(
        vec![
            entry("openai-chatgpt", "openai-chatgpt", "OAuth"),
            entry("gemini-code-assist", "gemini-code-assist", "OAuth"),
        ],
        "gemini",
    );
    let got = filtered(&l);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].provider, "gemini-code-assist");
}

#[test]
fn test_filtered_is_case_insensitive() {
    let l = state(
        vec![entry("openai-chatgpt", "OpenAI-ChatGPT", "OAuth")],
        "chatgpt",
    );
    assert_eq!(filtered(&l).len(), 1);
}
