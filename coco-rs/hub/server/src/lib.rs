//! Simplified Event Hub server backed directly by local session JSONL files.

mod display;
pub mod local_store;
pub mod routes;
pub mod store;

pub use local_store::LocalSessionJsonStore;
pub use routes::AppState;
pub use routes::router;
pub use store::EventRow;
pub use store::EventStore;
pub use store::EventStoreError;
pub use store::InstanceRow;
pub use store::SearchQuery;
pub use store::SessionRow;
