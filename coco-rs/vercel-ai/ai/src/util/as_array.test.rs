use super::*;
use std::sync::Arc;

#[test]
fn test_as_array_with_some() {
    let result = as_array(Some(Arc::new(42)));
    assert_eq!(result.len(), 1);
    assert_eq!(*result[0], 42);
}

#[test]
fn test_as_array_with_none() {
    let result: Vec<Arc<i32>> = as_array(None);
    assert!(result.is_empty());
}

#[test]
fn test_as_vec_with_single() {
    let result = as_vec(Some(Either::Left(42)));
    assert_eq!(result, vec![42]);
}

#[test]
fn test_as_vec_with_vec() {
    let result = as_vec(Some(Either::Right(vec![1, 2, 3])));
    assert_eq!(result, vec![1, 2, 3]);
}

#[test]
fn test_as_vec_with_none() {
    let result: Vec<i32> = as_vec(None);
    assert!(result.is_empty());
}
