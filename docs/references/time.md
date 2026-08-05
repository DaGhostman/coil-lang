# `time` module

`use time::{timestamp, sleep_ms, format, parse};` — UTC wall clock (`timestamp`, `epoch`), `Period` arithmetic, `format` / `parse` (strftime-style), monotonic `instant_now` / `elapsed_*`, and `sleep_ms`. Errors use `TimeError` inside `prelude::Result`. File bytes are not handled here; use `io` streams.

---
