use super::*;

#[tokio::test]
async fn test_hooks_no_args() {
    let result = handler("".to_string()).await.unwrap();
    // Either shows configured hooks or help text
    assert!(!result.is_empty());
}

/// The config example this command prints must parse with the real loader.
///
/// It did not. `/hooks` advertised Claude Code's nested shape — a matcher object
/// wrapping its own `hooks` array — while the loader expects the handler inline.
/// Anyone who followed the instructions got a config that fails to parse, and
/// because one bad entry fails the whole settings source, every other hook in
/// that file silently stopped firing. Nothing tied the two sides together, so
/// this went unnoticed. Now the printed text is the thing under test.
#[tokio::test]
async fn test_printed_config_example_parses_with_the_real_loader() {
    let out = handler("".to_string()).await.expect("handler");
    let Some(example) = extract_json_object(&out) else {
        // The example only prints in the empty state; a machine with hooks
        // configured takes the other branch and has nothing to check.
        return;
    };

    let parsed: serde_json::Value =
        serde_json::from_str(&example).expect("printed example must be valid JSON");
    let hooks = parsed
        .get("hooks")
        .expect("printed example must have a 'hooks' key");

    let loaded = coco_hooks::load_hooks_from_config(hooks, coco_types::HookScope::User)
        .expect("printed example must parse with the loader users will hit");
    assert!(
        !loaded.is_empty(),
        "printed example parsed but produced no hooks"
    );
}

/// Pull the first brace-balanced JSON object out of the command's prose output.
fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    for (offset, ch) in text[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..start + offset + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}
