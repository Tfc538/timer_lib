use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use timer_lib::Timer;
use tokio::runtime::Runtime;

fn bench_timer_start_and_join(c: &mut Criterion) {
    let runtime = Runtime::new().expect("runtime");
    let mut group = c.benchmark_group("timer_start_and_join");

    for delay_ms in [1_u64, 5, 10] {
        group.bench_with_input(
            BenchmarkId::from_parameter(delay_ms),
            &delay_ms,
            |b, delay_ms| {
                b.to_async(&runtime).iter(|| async move {
                    let timer = Timer::new();
                    let _ = timer
                        .start_once(Duration::from_millis(*delay_ms), || async { Ok(()) })
                        .await
                        .expect("start timer");
                    timer.join().await.expect("join timer");
                });
            },
        );
    }

    group.finish();
}

fn bench_timer_event_delivery(c: &mut Criterion) {
    let runtime = Runtime::new().expect("runtime");
    c.bench_function("timer_event_delivery", |b| {
        b.to_async(&runtime).iter(|| async {
            let timer = Timer::new();
            let mut events = timer.subscribe();
            let _ = timer
                .start_once(Duration::from_millis(1), || async { Ok(()) })
                .await
                .expect("start timer");

            let _ = events.wait_started().await.expect("started event");
            let _ = events.wait_tick().await.expect("tick event");
            let _ = events.wait_finished().await.expect("finished event");
        });
    });
}

criterion_group!(
    timer_benches,
    bench_timer_start_and_join,
    bench_timer_event_delivery
);
criterion_main!(timer_benches);
