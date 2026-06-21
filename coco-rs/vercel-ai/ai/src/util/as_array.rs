//! Convert a value or array to an array.

use std::sync::Arc;

/// Convert an Arc<dyn Trait> or Vec<Arc<dyn Trait>> to a Vec.
///
/// This is useful for middleware that can accept either a single item or a list.
pub fn as_array<T: Clone>(value: Option<Arc<T>>) -> Vec<Arc<T>> {
    match value {
        Some(v) => vec![v],
        None => vec![],
    }
}

/// Convert an optional single item or vector into a vector.
///
/// - If `None`, returns an empty vector.
/// - If `Some(single)`, returns a vector with that single item.
/// - If `Some(vec)`, returns the vector as-is.
pub fn as_vec<T>(value: Option<Either<T, Vec<T>>>) -> Vec<T> {
    match value {
        Some(Either::Left(v)) => vec![v],
        Some(Either::Right(v)) => v,
        None => vec![],
    }
}

/// A value that can be either a single item or a vector.
#[derive(Debug, Clone)]
pub enum Either<T, V> {
    /// A single item.
    Left(T),
    /// A vector of items.
    Right(V),
}

#[cfg(test)]
#[path = "as_array.test.rs"]
mod tests;
