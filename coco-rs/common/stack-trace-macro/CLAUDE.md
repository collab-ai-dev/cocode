# coco-stack-trace-macro

Proc macro `#[stack_trace_debug]` for snafu error enums. Generates `StackError` + `Debug` impls that render a virtual stack trace from per-variant `location` (`#[snafu(implicit)]`), `source`, and `error` fields.

## Key API

- `#[stack_trace_debug]` — attribute placed **before** `#[derive(Snafu)]` on an enum.

Each variant is analyzed for:
- `location: Location` — captured at error creation via `#[snafu(implicit)]`
- `source: T` — internal source that also implements `StackError` (recursive)
- `error: T` — external `std::error::Error` cause

Generated `Debug` produces one stack frame per layer, showing the correct file:line from error creation site.

## Dependencies

None (used by `coco-error` which re-exports it).
