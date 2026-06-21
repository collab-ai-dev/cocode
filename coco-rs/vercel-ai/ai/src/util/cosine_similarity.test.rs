//! Tests for cosine_similarity.rs

use super::*;

#[test]
fn test_identical_vectors() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    let sim = cosine_similarity(&a, &b).unwrap();
    assert!((sim - 1.0).abs() < 1e-6);
}

#[test]
fn test_orthogonal_vectors() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let sim = cosine_similarity(&a, &b).unwrap();
    assert!((sim - 0.0).abs() < 1e-6);
}

#[test]
fn test_opposite_vectors() {
    let a = vec![1.0, 0.0];
    let b = vec![-1.0, 0.0];
    let sim = cosine_similarity(&a, &b).unwrap();
    assert!((sim - (-1.0)).abs() < 1e-6);
}

#[test]
fn test_empty_vectors() {
    let a: Vec<f32> = vec![];
    let b: Vec<f32> = vec![];
    assert!(cosine_similarity(&a, &b).is_none());
}

#[test]
fn test_different_lengths() {
    let a = vec![1.0, 2.0];
    let b = vec![1.0, 2.0, 3.0];
    assert!(cosine_similarity(&a, &b).is_none());
}
