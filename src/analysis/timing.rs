//! Index timings.
//!
//! This module records the time of each analysis stage and each LSP request
//! when the `TEKDEK_TIMINGS` environment variable is set. `report` prints the
//! records to standard error. Without the variable, each call returns at once
//! and records nothing. The records are shared, so language threads add to the
//! same stage list and the same request statistics.

use std::{
    collections::BTreeMap,
    env, mem,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

static TIMINGS: OnceLock<Option<Mutex<Timings>>> = OnceLock::new();

/// Starts one measurement, or returns `None` when timings are off.
pub(crate) fn start() -> Option<Instant> {
    timings().map(|_| Instant::now())
}

/// Records one stage that started at `started`.
pub(crate) fn record_stage(started: Option<Instant>, name: impl FnOnce() -> String) {
    if let (Some(started), Some(timings)) = (started, timings())
        && let Ok(mut timings) = timings.lock()
    {
        timings.stages.push((name(), started.elapsed()));
    }
}

/// Records one LSP request that started at `started`.
pub(crate) fn record_request(started: Option<Instant>, method: &'static str) {
    if let (Some(started), Some(timings)) = (started, timings())
        && let Ok(mut timings) = timings.lock()
    {
        timings.add_request(method, started.elapsed());
    }
}

/// Prints the total time since `started`, each stage, and each request method
/// to standard error, then clears the records.
pub(crate) fn report(started: Option<Instant>) {
    if let (Some(started), Some(timings)) = (started, timings())
        && let Ok(mut timings) = timings.lock()
    {
        for line in mem::take(&mut *timings).lines(started.elapsed()) {
            eprintln!("{line}");
        }
    }
}

fn timings() -> Option<&'static Mutex<Timings>> {
    TIMINGS
        .get_or_init(|| env::var_os("TEKDEK_TIMINGS").map(|_| Mutex::default()))
        .as_ref()
}

#[derive(Default)]
struct Timings {
    stages: Vec<(String, Duration)>,
    requests: BTreeMap<&'static str, RequestTimes>,
}

#[derive(Default)]
struct RequestTimes {
    count: u32,
    total: Duration,
    slowest: Duration,
}

impl Timings {
    fn add_request(&mut self, method: &'static str, elapsed: Duration) {
        let times = self.requests.entry(method).or_default();
        times.count += 1;
        times.total += elapsed;
        times.slowest = times.slowest.max(elapsed);
    }

    /// Returns the report lines: total, stages in the order they end, then
    /// request methods in name order.
    fn lines(&self, total: Duration) -> Vec<String> {
        let mut lines = vec![format!("timings: total {}", seconds(total))];
        lines.extend(
            self.stages
                .iter()
                .map(|(name, elapsed)| format!("  stage {name}: {}", seconds(*elapsed))),
        );
        lines.extend(self.requests.iter().map(|(method, times)| {
            format!(
                "  request {method}: {} requests, {} total, {} slowest",
                times.count,
                seconds(times.total),
                seconds(times.slowest)
            )
        }));
        lines
    }
}

fn seconds(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Timings;

    #[test]
    fn reports_stages_and_the_count_total_and_slowest_request_of_each_method() {
        let mut timings = Timings::default();
        timings
            .stages
            .push(("load".to_owned(), Duration::from_millis(1500)));
        timings.add_request("textDocument/definition", Duration::from_millis(20));
        timings.add_request("textDocument/definition", Duration::from_millis(5));
        timings.add_request("initialize", Duration::from_millis(300));

        assert_eq!(
            timings.lines(Duration::from_secs(2)),
            [
                "timings: total 2.000s",
                "  stage load: 1.500s",
                "  request initialize: 1 requests, 0.300s total, 0.300s slowest",
                "  request textDocument/definition: 2 requests, 0.025s total, 0.020s slowest",
            ]
        );
    }
}
