# timer-lib

`timer-lib` is a Tokio-based timer crate for one-shot and recurring async work.

It is built around a small set of handle types:

- `Timer` starts, pauses, resumes, stops, cancels, and joins runs.
- `TimerBuilder` reduces setup boilerplate for common configurations.
- `TimerRegistry` tracks timers by ID and provides bulk operations.
- `TimerEvents` exposes lossy lifecycle broadcasts.
- `TimerCompletion` exposes lossless completed-run delivery.

## Features

- One-shot and recurring timers
- Pause, resume, graceful stop, and immediate cancel
- Dynamic interval adjustment for live runs
- Run outcomes and execution statistics
- Broadcast lifecycle events plus lossless completion waiting
- Registry helpers for managing many timers
- Closure-first API with optional trait-based callbacks

## Installation

```toml
[dependencies]
timer-lib = "0.2.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
```

## Quick Start

```rust
use std::time::Duration;
use timer_lib::{Timer, TimerFinishReason};

#[tokio::main]
async fn main() {
    let timer = Timer::new();

    timer
        .start_once(Duration::from_millis(250), || async {
            println!("ran once");
            Ok(())
        })
        .await
        .unwrap();

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}
```

## Recurring Timers

```rust
use std::time::Duration;
use timer_lib::{Timer, TimerFinishReason};

#[tokio::main]
async fn main() {
    let timer = Timer::recurring(Duration::from_secs(1))
        .expiration_count(3)
        .start(|| async {
            println!("tick");
            Ok(())
        })
        .await
        .unwrap();

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
    assert_eq!(outcome.statistics.execution_count, 3);
}
```

## Events And Completion

Use `subscribe()` when you want a best-effort event stream and `completion()` when you need to reliably observe the final outcome of a run.

```rust
use std::time::Duration;
use timer_lib::{Timer, TimerEvent};

#[tokio::main]
async fn main() {
    let timer = Timer::new();
    let mut events = timer.subscribe();
    let mut completion = timer.completion();

    let run_id = timer
        .start_once(Duration::from_millis(100), || async { Ok(()) })
        .await
        .unwrap();

    if let Some(TimerEvent::Started { run_id: seen, .. }) = events.wait_started().await {
        assert_eq!(seen, run_id);
    }

    let outcome = completion.wait_for_run(run_id).await.unwrap();
    println!("finished: {:?}", outcome.reason);
}
```

## Managing Many Timers

```rust
use std::time::Duration;
use timer_lib::TimerRegistry;

#[tokio::main]
async fn main() {
    let registry = TimerRegistry::new();

    let (timer_id, timer) = registry
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();

    assert!(registry.contains(timer_id).await);
    let _ = timer.join().await.unwrap();

    let completed = registry.join_all().await;
    assert!(!completed.is_empty());
}
```

## Callback Styles

The simplest API uses closures:

```rust
# use std::time::Duration;
# use timer_lib::Timer;
# async fn demo() {
let timer = Timer::new();
let _ = timer
    .start_once(Duration::from_secs(1), || async { Ok(()) })
    .await;
# }
```

If you need a reusable callback type, implement `TimerCallback`:

```rust
use async_trait::async_trait;
use timer_lib::{TimerCallback, TimerError};

struct MyCallback;

#[async_trait]
impl TimerCallback for MyCallback {
    async fn execute(&self) -> Result<(), TimerError> {
        Ok(())
    }
}
```

## Current Scope

`timer-lib` currently targets Tokio runtimes. It does not provide cron scheduling or `async-std` support.

The next major documentation pass should live on docs.rs. For now, the most complete usage sample is [examples/feature_showcase.rs](examples/feature_showcase.rs).
