use coco_app_server::ConnectionKey;

use crate::session_start::SessionStartInput;

#[derive(Debug, Clone)]
pub(crate) enum LocalSessionOperation {
    Close {
        connection: ConnectionKey,
        target: coco_types::SessionCloseTarget,
    },
}

pub(crate) struct SessionReplaceInput {
    pub(crate) source: coco_types::InteractiveTarget,
    pub(crate) destination: SessionReplaceDestination,
}

pub(crate) enum SessionReplaceDestination {
    // Boxed: `SessionStartInput` is much larger than the other variants.
    Fresh(Box<SessionStartInput>),
    Resume(coco_types::SessionTarget),
    Clear,
}
