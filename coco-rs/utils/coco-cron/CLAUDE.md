# coco-cron

Minimal cron parsing, next-run computation, and the pure (I/O-free) scheduler tick core.

## Key API

| Item | Purpose |
|------|---------|
| `parse_cron_expression` / `is_valid_cron_expression` | 5-field cron → expanded `CronFields`; `None` on invalid |
| `next_cron_run_ms` / `compute_next_cron_run` | Next fire strictly after `from`; `None` if no match within 366 days |
| `cron_to_human` | Readable rendering of common patterns; falls back to the raw cron string |
| `CronTickState::tick` | Pure per-tick fire decisions (`DueFire`); caller performs the side effects |
| `find_missed` | Startup scan: one-shot tasks whose time passed while the process was down |
| `RECURRING_MAX_AGE_MS` / `is_recurring_task_aged` | 7-day recurring auto-expiry (`0` = never); aged tasks fire once then are removed |

## Invariants

- Cron subset only: wildcard, `N`, `*/N`, `N-M[/S]`, `,` lists. No `L`, `W`, `?`, or name aliases; `7` is a Sunday alias in day-of-week.
- All times are the process's **local** timezone — `"0 9 * * *"` is 9am wherever the CLI runs. DST spring-forward gaps skip fixed-hour matches (vixie-cron behavior).
- `tick` reschedules recurring tasks from `now`, not the matched instant — no rapid catch-up bursts. `find_missed` covers one-shot tasks only; recurring tasks fire on the first `tick` pass.
