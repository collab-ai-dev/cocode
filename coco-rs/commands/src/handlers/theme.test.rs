use super::*;
use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

#[tokio::test]
async fn no_args_opens_picker() {
    let result = ThemeHandler.execute_command("").await.unwrap();
    assert!(matches!(
        result,
        CommandResult::OpenDialog(DialogSpec::ThemePicker)
    ));
}

#[tokio::test]
async fn whitespace_args_open_picker() {
    let result = ThemeHandler.execute_command("   ").await.unwrap();
    assert!(matches!(
        result,
        CommandResult::OpenDialog(DialogSpec::ThemePicker)
    ));
}

#[tokio::test]
async fn named_arg_is_ignored_and_still_opens_picker() {
    // `/theme` ignores any argument and always renders the picker, so a
    // stray `/theme dark` must still open the overlay (never report inline).
    let result = ThemeHandler.execute_command("dark").await.unwrap();
    assert!(matches!(
        result,
        CommandResult::OpenDialog(DialogSpec::ThemePicker)
    ));
}
