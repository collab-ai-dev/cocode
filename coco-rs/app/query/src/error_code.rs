//! Bridge between `coco_error::StatusCategory` (the internal error
//! taxonomy) and `coco_types::ErrorCode` (the wire-stable category
//! emitted on `TurnEnded(Failed)`). Both enums live one layer apart —
//! `coco-error` and `coco-types` deliberately don't depend on each
//! other to keep the common-layer DAG flat — so this seam lives here
//! in `coco-query`, the lowest crate that already depends on both.
//!
//! Public surface is [`error_code_from_category`] (for callers holding
//! a typed `StatusCategory`) and [`error_code_from_boxed_error`] (for
//! callers projecting from `BoxedError`). The `StatusCode → ErrorCode`
//! intermediate hop is crate-private — production callers either know
//! the category up front or are working with a `BoxedError`.

use coco_error::BoxedError;
use coco_error::status_code::StatusCategory;
use coco_error::status_code::StatusCode;
use coco_types::ErrorCode;

/// Map a `StatusCategory` to a wire-stable `ErrorCode`.
///
/// `Success` collapses to `Unknown` — emitters reaching this fn
/// through the error path with `Success` are buggy; we surface that
/// as an honest "I don't know" rather than misclassifying as
/// `Common`. Hook-policy blocks should use [`ErrorCode::HookBlocked`]
/// directly at the call site rather than routing through here.
pub fn error_code_from_category(category: StatusCategory) -> ErrorCode {
    match category {
        StatusCategory::Success => ErrorCode::Unknown,
        StatusCategory::Common => ErrorCode::Common,
        StatusCategory::Input => ErrorCode::Input,
        StatusCategory::IO => ErrorCode::Io,
        StatusCategory::Network => ErrorCode::Network,
        StatusCategory::Auth => ErrorCode::Auth,
        StatusCategory::Config => ErrorCode::Config,
        StatusCategory::Provider => ErrorCode::Provider,
        StatusCategory::Resource => ErrorCode::Resource,
        StatusCategory::SystemReminder => ErrorCode::SystemReminder,
    }
}

/// Map a `StatusCode` to its wire-stable `ErrorCode` by reading the
/// associated category. Crate-private: the only external caller is
/// [`error_code_from_boxed_error`]; everyone else either has the
/// category already or is starting from a `BoxedError`.
fn error_code_from_status(status: StatusCode) -> ErrorCode {
    error_code_from_category(status.category())
}

/// Read the category from a `BoxedError` and project to `ErrorCode`.
pub fn error_code_from_boxed_error(err: &BoxedError) -> ErrorCode {
    error_code_from_status(err.status_code())
}
