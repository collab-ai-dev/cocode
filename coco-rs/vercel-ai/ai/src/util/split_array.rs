//! Array splitting utility.
//!
//! This module provides utilities for splitting arrays into chunks.

/// Split an array into chunks of a given size.
///
/// # Arguments
///
/// * `array` - The array to split.
/// * `chunk_size` - The size of each chunk.
///
/// # Returns
///
/// A vector of chunks.
pub fn split_array<T: Clone>(array: &[T], chunk_size: usize) -> Vec<Vec<T>> {
    if chunk_size == 0 {
        return vec![array.to_vec()];
    }

    array.chunks(chunk_size).map(<[T]>::to_vec).collect()
}

/// Split an array into N approximately equal parts.
///
/// # Arguments
///
/// * `array` - The array to split.
/// * `parts` - The number of parts.
///
/// # Returns
///
/// A vector of parts.
pub fn split_into_parts<T: Clone>(array: &[T], parts: usize) -> Vec<Vec<T>> {
    if parts == 0 {
        return vec![array.to_vec()];
    }

    let len = array.len();
    let part_size = len / parts;
    let remainder = len % parts;

    let mut result = Vec::with_capacity(parts);
    let mut start = 0;

    for i in 0..parts {
        let extra = if i < remainder { 1 } else { 0 };
        let end = start + part_size + extra;
        result.push(array[start..end.min(len)].to_vec());
        start = end;
    }

    result
}

/// Split an array at a given index.
///
/// # Arguments
///
/// * `array` - The array to split.
/// * `index` - The index to split at.
///
/// # Returns
///
/// Two arrays: the first containing elements before the index, the second containing elements at and after.
pub fn split_at<T: Clone>(array: &[T], index: usize) -> (Vec<T>, Vec<T>) {
    if index >= array.len() {
        return (array.to_vec(), Vec::new());
    }

    (array[..index].to_vec(), array[index..].to_vec())
}

/// Split an array at multiple indices.
///
/// # Arguments
///
/// * `array` - The array to split.
/// * `indices` - The indices to split at.
///
/// # Returns
///
/// A vector of parts.
pub fn split_at_indices<T: Clone>(array: &[T], indices: &[usize]) -> Vec<Vec<T>> {
    if indices.is_empty() {
        return vec![array.to_vec()];
    }

    let mut sorted_indices: Vec<usize> = indices.to_vec();
    sorted_indices.sort();
    sorted_indices.dedup();

    let mut result = Vec::new();
    let mut start = 0;

    for &index in &sorted_indices {
        if index > start && index <= array.len() {
            result.push(array[start..index].to_vec());
            start = index;
        }
    }

    if start < array.len() {
        result.push(array[start..].to_vec());
    }

    result
}

/// Take the first N elements from an array.
///
/// # Arguments
///
/// * `array` - The array.
/// * `n` - The number of elements to take.
///
/// # Returns
///
/// The first N elements.
pub fn take_first<T: Clone>(array: &[T], n: usize) -> Vec<T> {
    array.iter().take(n).cloned().collect()
}

/// Take the last N elements from an array.
///
/// # Arguments
///
/// * `array` - The array.
/// * `n` - The number of elements to take.
///
/// # Returns
///
/// The last N elements.
pub fn take_last<T: Clone>(array: &[T], n: usize) -> Vec<T> {
    let start = array.len().saturating_sub(n);
    array[start..].to_vec()
}
