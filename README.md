# TimerLib

A feature-rich timer library for Rust with support for one-time, recurring, and scheduled timers. **TimerLib** is designed to be robust, flexible, and easy to use with advanced features like pause/resume functionality, dynamic interval adjustments, and timer statistics. Under the hood, it uses a combination of `tokio` and `async-std` to provide a seamless async/await experience.

---

## Features

- **One-Time Timers**: Execute a task after a specified delay.
- **Recurring Timers**: Schedule tasks at regular intervals.
- **Pause and Resume**: Pause and resume timers dynamically.
- **Dynamic Interval Adjustment**: Change intervals on-the-fly for recurring timers.
- **Timer Statistics**: Track execution counts and elapsed time.
- **Thread Safety**: Fully compatible with multi-threaded environments.
- **Async/Futures Integration**: Built to work seamlessly with async/await.
- **Error Handling**: Comprehensive error handling and fallback mechanisms.

---

## Comparison

| Feature | TimerLib | tokio | async-std
| --- | --- | --- | --- |
| **One-Time Timers** | ✓ | ✓ | ✓ |
| **Recurring Timers** | ✓ | ✓ | ✓ |
| **Pause and Resume** | ✓ | ✗ | ✗ |
| **Dynamic Interval Adjustment** | ✓ | ✗ | ✗ |
| **Timer Statistics** | ✓ | ✗ | ✗ |
| **Thread Safety** | ✓ | ✓ | ✓ |
| **Async/Futures Integration** | ✓ | ✓ | ✓ |
| **Error Handling** | ✓ | ✓ | ✓ |
| **Performance** | High | High | High |

---

## Installation

Add TimerLib to your `Cargo.toml`:

```toml
[dependencies]
timer-lib = "1.0.0"
```

## Usage

Here’s a quick example of using **TimerLib** to schedule a one-time task:

```rust
use timer-lib::{Timer, TimerCallback};
use async_trait::async_trait;

struct MyCallback;

#[async_trait]
impl TimerCallback for MyCallback {
    async fn execute(&self) {
        println!("Timer executed!");
    }
}

#[tokio::main]
async fn main() {
    let mut timer = Timer::new();
    timer.start_once(std::time::Duration::from_secs(2), MyCallback).await.unwrap();
}
```

### For recurring timers:
```rust
use timer-lib::{Timer, TimerCallback};
use async_trait::async_trait;

struct RecurringCallback;

#[async_trait]
impl TimerCallback for RecurringCallback {
    async fn execute(&self) {
        println!("Recurring timer executed!");
    }
}

#[tokio::main]
async fn main() {
    let mut timer = Timer::new();
    timer.start_recurring(std::time::Duration::from_secs(5), RecurringCallback, None).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(15)).await; // Let it run for 15 seconds
    timer.stop(); // Stop the timer
}
```

For more examples, check the [documentation](https://docs.rs/timer-lib).

---

## Documentation

Detailed documentation is available on [docs.rs](https://docs.rs/timer-lib).

---

## Contributing

Contributions are welcome! Please read the [CONTRIBUTING.md](CONTRIBUTING.md) file for guidelines.

---

## License

This project is licensed under the **MIT License**. See the [LICENSE](LICENSE) file for details.
