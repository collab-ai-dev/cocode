use super::*;

#[test]
fn test_split_array() {
    let array = vec![1, 2, 3, 4, 5, 6];
    let chunks = split_array(&array, 2);

    assert_eq!(chunks, vec![vec![1, 2], vec![3, 4], vec![5, 6]]);
}

#[test]
fn test_split_array_uneven() {
    let array = vec![1, 2, 3, 4, 5];
    let chunks = split_array(&array, 2);

    assert_eq!(chunks, vec![vec![1, 2], vec![3, 4], vec![5]]);
}

#[test]
fn test_split_into_parts() {
    let array = vec![1, 2, 3, 4, 5, 6];
    let parts = split_into_parts(&array, 3);

    assert_eq!(parts.len(), 3);
    assert_eq!(parts.iter().map(|p| p.len()).sum::<usize>(), 6);
}

#[test]
fn test_split_at() {
    let array = vec![1, 2, 3, 4, 5];
    let (left, right) = split_at(&array, 2);

    assert_eq!(left, vec![1, 2]);
    assert_eq!(right, vec![3, 4, 5]);
}

#[test]
fn test_split_at_indices() {
    let array = vec![1, 2, 3, 4, 5, 6];
    let parts = split_at_indices(&array, &[2, 4]);

    assert_eq!(parts, vec![vec![1, 2], vec![3, 4], vec![5, 6]]);
}

#[test]
fn test_take_first() {
    let array = vec![1, 2, 3, 4, 5];
    assert_eq!(take_first(&array, 3), vec![1, 2, 3]);
}

#[test]
fn test_take_last() {
    let array = vec![1, 2, 3, 4, 5];
    assert_eq!(take_last(&array, 3), vec![3, 4, 5]);
}