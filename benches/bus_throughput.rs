//! Criterion bench for the event bus.
//!
//! Target (per PRD NFR-4 / BUILD_PLAN TASK-101): > 500k events/sec with
//! three active subscribers. Run with `cargo bench --bench bus_throughput`.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rustotron::bus::{ClientId, Event, RequestId, new_bus};
use std::hint::black_box;
use tokio::sync::broadcast::error::TryRecvError;

/// Publisher emits `total_events` ResponseReceived into a bus with
/// `subscriber_count` drains. Each drain is pulled to empty between
/// publish bursts so the ring never lags. Measures publisher wall time.
fn run_fanout(total_events: usize, subscriber_count: usize) {
    // Bus capacity sized generously vs subscriber burst tolerance.
    let bus = new_bus(1024);
    let subscribers: Vec<_> = (0..subscriber_count).map(|_| bus.subscribe()).collect();

    // Publisher loop — every 256 sends, drain subscribers to keep the ring
    // from lagging. This approximates a realistic steady-state where
    // TUI / MCP / tail keep up with production traffic.
    let mut subscribers = subscribers;
    for i in 0..total_events {
        let event = if i % 2 == 0 {
            Event::ResponseReceived(RequestId::new())
        } else {
            Event::ClientConnected(ClientId::new())
        };
        // send fails only if no subscribers — we have subscriber_count > 0.
        let _ = bus.send(event);

        if i % 256 == 0 {
            for rx in &mut subscribers {
                loop {
                    match rx.try_recv() {
                        Ok(ev) => {
                            black_box(ev);
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Lagged(_)) => continue,
                        Err(TryRecvError::Closed) => break,
                    }
                }
            }
        }
    }
}

fn bench_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("bus_fanout");
    // 100k events × 3 subs is enough signal; criterion runs multiple iters.
    let events: usize = 100_000;
    group.throughput(Throughput::Elements(events as u64));

    for subs in [1_usize, 3, 8] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{subs}_subscribers")),
            &subs,
            |b, &subs| {
                b.iter(|| run_fanout(events, subs));
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_fanout);
criterion_main!(benches);
