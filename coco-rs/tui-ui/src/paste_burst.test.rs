use super::*;
use crossterm::event::KeyEvent;

fn ch(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// Feed `text` as individual key events `gap` apart, starting at `start`.
/// Returns the timestamp of the last key.
fn type_text(burst: &mut PasteBurst, text: &str, start: Instant, gap: Duration) -> Instant {
    let mut now = start;
    for c in text.chars() {
        let key = if c == '\n' { enter() } else { ch(c) };
        burst.observe(key, now);
        now += gap;
    }
    now - gap
}

#[test]
fn test_paste_burst_idle_tracker_is_not_bursting() {
    let burst = PasteBurst::new();
    assert!(!burst.is_bursting(Instant::now()));
}

#[test]
fn test_paste_burst_opens_window_for_paste_speed_input() {
    let mut burst = PasteBurst::new();
    let t0 = Instant::now();
    let last = type_text(&mut burst, "select", t0, ms(1));
    assert!(burst.is_bursting(last));
}

/// The defect this exists for: a multi-line paste on a terminal without
/// bracketed paste must not read its embedded newlines as submits.
#[test]
fn test_paste_burst_stays_open_across_embedded_newlines() {
    let mut burst = PasteBurst::new();
    let t0 = Instant::now();
    let last = type_text(&mut burst, "line one\nline two\nline three", t0, ms(1));
    assert!(burst.is_bursting(last));
}

/// Sustained fast human typing is around 10 characters per second; even a burst
/// digraph does not reach the threshold.
#[test]
fn test_paste_burst_ignores_fast_human_typing() {
    let mut burst = PasteBurst::new();
    let t0 = Instant::now();
    let last = type_text(&mut burst, "a fast typist", t0, ms(60));
    assert!(!burst.is_bursting(last));
}

/// The regression that a naive implementation ships: type a word quickly, pause
/// to think, then press Enter. That Enter must submit.
#[test]
fn test_paste_burst_enter_after_a_pause_is_not_a_paste() {
    let mut burst = PasteBurst::new();
    let t0 = Instant::now();
    let last = type_text(&mut burst, "hello", t0, ms(1));
    assert!(burst.is_bursting(last));

    let after_pause = last + BURST_IDLE_TIMEOUT + ms(1);
    burst.observe(enter(), after_pause);
    assert!(!burst.is_bursting(after_pause));
}

/// Enter alone can never open a window, however fast it is pressed.
#[test]
fn test_paste_burst_repeated_enter_never_opens_a_window() {
    let mut burst = PasteBurst::new();
    let mut now = Instant::now();
    for _ in 0..10 {
        burst.observe(enter(), now);
        assert!(!burst.is_bursting(now));
        now += ms(1);
    }
}

#[test]
fn test_paste_burst_window_expires_after_idle_timeout() {
    let mut burst = PasteBurst::new();
    let t0 = Instant::now();
    let last = type_text(&mut burst, "pasted", t0, ms(1));

    assert!(burst.is_bursting(last + BURST_IDLE_TIMEOUT - ms(1)));
    assert!(!burst.is_bursting(last + BURST_IDLE_TIMEOUT));
}

/// A slow character resets the run; the burst must not accumulate across a
/// human-scale pause in the middle.
#[test]
fn test_paste_burst_slow_character_restarts_the_run() {
    let mut burst = PasteBurst::new();
    let mut now = Instant::now();
    for c in "ab".chars() {
        burst.observe(ch(c), now);
        now += ms(1);
    }
    now += ms(500);
    for c in "cd".chars() {
        burst.observe(ch(c), now);
        now += ms(1);
    }
    assert!(!burst.is_bursting(now));
}

/// A deliberate chord means a human took over; the paste is done.
#[test]
fn test_paste_burst_modified_key_closes_the_window() {
    let mut burst = PasteBurst::new();
    let t0 = Instant::now();
    let last = type_text(&mut burst, "pasted", t0, ms(1));
    assert!(burst.is_bursting(last));

    burst.observe(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        last,
    );
    assert!(!burst.is_bursting(last));
}

/// Arrows and other navigation keys cannot appear inside a paste.
#[test]
fn test_paste_burst_navigation_key_closes_the_window() {
    let mut burst = PasteBurst::new();
    let t0 = Instant::now();
    let last = type_text(&mut burst, "pasted", t0, ms(1));

    burst.observe(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), last);
    assert!(!burst.is_bursting(last));
}

/// Pasted uppercase and symbols arrive with SHIFT on many terminals.
#[test]
fn test_paste_burst_shifted_characters_still_count() {
    let mut burst = PasteBurst::new();
    let mut now = Instant::now();
    for c in "SELECT".chars() {
        burst.observe(KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT), now);
        now += ms(1);
    }
    assert!(burst.is_bursting(now));
}

/// Holding a character key is the other way to produce a fast stream, and
/// crossterm reports it as `Repeat` when the kitty protocol is active.
#[test]
fn test_paste_burst_key_auto_repeat_is_not_a_paste() {
    let mut burst = PasteBurst::new();
    let mut now = Instant::now();
    for _ in 0..10 {
        let mut key = ch('x');
        key.kind = KeyEventKind::Repeat;
        burst.observe(key, now);
        now += ms(1);
    }
    assert!(!burst.is_bursting(now));
}
