# Changelog

All notable changes to this project will be documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

*None*

## [0.3.0] - 2026-03-10

### Added

- `RecurringSchedule` as the public recurring-timer configuration type.
- `RecurringCadence` with fixed-delay and fixed-rate scheduling modes.

### Changed

- Made recurring timer startup APIs schedule-based instead of interval-based.
- Moved recurring-only options like initial delay and expiration count onto `RecurringSchedule`.
- Updated tests, examples, and crate docs to reflect the new recurring API shape.

## [0.2.1] - 2026-03-10

### Added

- Recurring timer support for an explicit initial delay before the first tick.
- Per-callback timeout handling with dedicated timeout errors.
- Retry policy support for callback failures.
- `TimerRegistry` helpers for pausing and resuming tracked timers.
- `TimerEvents` helpers for waiting on paused, resumed, stopped, and cancelled transitions.

## [0.2.0] - 2026-03-10

### Added

- `TimerRegistry` as the primary registry abstraction.
- Graceful stop, immediate cancel, join outcomes, and lifecycle event streams.
- Lossless completion waiting via `TimerCompletion`.
- Builder-based timer startup with paused-start and event-control options.
- Stress, public API, weird-edge, and benchmark coverage.

### Changed

- Raised the minimum resolved `bytes` dependency to 1.11.1.
- Reworked the runtime around explicit control commands and async-native internals.
- Made callback failures observable through outcomes, statistics, and event delivery.
- Removed manual `unsafe impl` usage from the timer and registry internals.
- Updated the README, examples, and crate docs to match the current Tokio-based API.
