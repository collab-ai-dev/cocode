use super::*;
use crate::settings_registry::SettingId;

#[test]
fn test_filter_resets_selection_and_matches_registry_metadata() {
    let mut state = SettingsPanelState {
        selected: 4,
        ..Default::default()
    };
    for c in "clipboard".chars() {
        state.insert_filter(c);
    }

    assert_eq!(state.selected, 0);
    let matches = state.filtered_settings();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, SettingId::CopyFullResponse);

    state.filter_backspace();
    assert_eq!(state.selected, 0);
}
