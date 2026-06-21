# coco-utils-sleep-inhibitor

Prevent machine idle sleep while a turn is running. Per-platform backend; no-op on unsupported OSes.

## Key Types

- `SleepInhibitor::new(enabled: bool)` — constructor
- `set_turn_running(bool)` — acquire / release on transition
- `is_turn_running() -> bool`
- Platform modules: `macos` (IOKit `PowerCreateRequest`), `linux_inhibitor` (`systemd-inhibit` / `gnome-session-inhibit`), `windows_inhibitor` (`PowerSetRequest`), `dummy` (other)
