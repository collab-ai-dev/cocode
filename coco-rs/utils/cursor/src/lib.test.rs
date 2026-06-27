//! Tests for cursor operations.

use crate::Cursor;
use crate::word_at;

#[test]
fn test_basic_movement() {
    let mut c = Cursor::new();
    assert_eq!(c.pos, 0);

    c.right(10);
    assert_eq!(c.pos, 1);

    c.left();
    assert_eq!(c.pos, 0);

    c.left(); // clamped at 0
    assert_eq!(c.pos, 0);
}

#[test]
fn test_home_end() {
    let mut c = Cursor::at(5);
    c.home();
    assert_eq!(c.pos, 0);

    c.end(10);
    assert_eq!(c.pos, 10);
}

#[test]
fn test_word_left() {
    let mut c = Cursor::at(11);
    c.word_left("hello world");
    assert_eq!(c.pos, 6); // start of "world"

    c.word_left("hello world");
    assert_eq!(c.pos, 0); // start of "hello"
}

#[test]
fn test_word_right() {
    let mut c = Cursor::new();
    c.word_right("hello world");
    assert_eq!(c.pos, 6); // after "hello "

    c.word_right("hello world");
    assert_eq!(c.pos, 11); // end
}

#[test]
fn test_kill_to_end() {
    let mut c = Cursor::at(5);
    let result = c.kill_to_end("hello world");
    assert!(result.is_some());
    let r = result.unwrap();
    assert_eq!(r.killed, " world");
    assert_eq!(r.start, 5);
    assert_eq!(r.end, 11);
}

#[test]
fn test_kill_accumulation() {
    let mut c = Cursor::new();

    // First kill
    c.kill_to_end("abc\ndef");
    assert_eq!(c.yank(), Some("abc"));

    // Second consecutive kill accumulates
    c.pos = 0;
    // Simulate: last_was_kill is still true
    c.kill_to_end("remaining");
    let yanked = c.yank().unwrap();
    assert!(yanked.contains("remaining"));
}

#[test]
fn test_yank_empty() {
    let c = Cursor::new();
    assert_eq!(c.kill_ring.len(), 0);
}

#[test]
fn test_word_at() {
    assert_eq!(word_at("hello world", 0), Some((0, 5)));
    assert_eq!(word_at("hello world", 6), Some((6, 11)));
    assert_eq!(word_at("hello world", 5), None); // space
}

#[test]
fn test_to_byte_offset() {
    let c = Cursor::at(2);
    assert_eq!(c.to_byte_offset("hello"), 2);

    // UTF-8 multibyte
    let c = Cursor::at(1);
    assert_eq!(c.to_byte_offset("é世界"), 2); // 'é' is 2 bytes
}

#[test]
fn test_right_clamped() {
    let mut c = Cursor::at(5);
    c.right(5); // at end
    assert_eq!(c.pos, 5); // clamped
}
