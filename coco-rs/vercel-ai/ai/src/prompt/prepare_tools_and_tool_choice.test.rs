use super::*;

#[test]
fn test_prepare_tools_and_tool_choice_no_tools() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let result = prepare_tools_and_tool_choice(call_options, None, None);

    assert!(result.tools.is_none());
    assert!(result.tool_choice.is_none());
}

#[test]
fn test_prepare_tools_and_tool_choice_with_choice() {
    let call_options = LanguageModelV4CallOptions::new(vec![]);
    let result =
        prepare_tools_and_tool_choice(call_options, None, Some(&LanguageModelV4ToolChoice::auto()));

    assert_eq!(result.tool_choice, Some(LanguageModelV4ToolChoice::auto()));
}

#[test]
fn test_prepare_tool_definitions_empty() {
    let registry = ToolRegistry::new();
    let definitions = prepare_tool_definitions(&registry);
    assert!(definitions.is_empty());
}

#[test]
fn test_determine_tool_choice_user_specified() {
    let choice = LanguageModelV4ToolChoice::auto();
    let result = determine_tool_choice(None, Some(&choice), false);

    assert_eq!(result, Some(choice));
}

#[test]
fn test_determine_tool_choice_auto_no_tools() {
    let result = determine_tool_choice(None, None, true);
    assert!(result.is_none());
}

#[test]
fn test_is_tool_call_required() {
    assert!(is_tool_call_required(Some(
        &LanguageModelV4ToolChoice::required()
    )));
    assert!(!is_tool_call_required(Some(
        &LanguageModelV4ToolChoice::auto()
    )));
    assert!(!is_tool_call_required(Some(
        &LanguageModelV4ToolChoice::none()
    )));
    assert!(!is_tool_call_required(None));
}

#[test]
fn test_is_tool_call_disabled() {
    assert!(is_tool_call_disabled(false, None));
    assert!(is_tool_call_disabled(
        true,
        Some(&LanguageModelV4ToolChoice::none())
    ));
    assert!(!is_tool_call_disabled(
        true,
        Some(&LanguageModelV4ToolChoice::auto())
    ));
    assert!(!is_tool_call_disabled(true, None));
}
