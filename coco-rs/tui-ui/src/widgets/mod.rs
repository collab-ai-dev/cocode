//! Pure, domain-free ratatui widgets. Each is constructed from plain values /
//! view models (never `AppState`) and implements `ratatui::widgets::Widget` or
//! renders into a `Buffer` / returns `Line`s.

pub mod diff_display;
pub mod notification;
pub mod question;
pub mod select_list;
pub mod spinner_verbs;
pub mod status_indicator;
pub mod textarea;

pub use question::ActionRow;
pub use question::ChoiceRow;
pub use question::InputRow;
pub use question::NavTab;
pub use question::QuestionHeader;
pub use question::QuestionNav;
pub use question::QuestionRow;
pub use question::QuestionView;
pub use question::QuestionWidget;
pub use question::RowMark;
pub use question::SubmitNavTab;
pub use select_list::SelectItem;
pub use select_list::SelectListStyle;
pub use select_list::render_select_list;
pub use status_indicator::StatusIndicator;
pub use status_indicator::StatusIndicatorView;
pub use textarea::BolBehavior;
pub use textarea::EolBehavior;
pub use textarea::TextArea;
