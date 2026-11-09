use std::time::Duration;

use tokio::time::Instant;

#[derive(Debug, Clone)]
pub struct Schedule {
    interval: Duration,
    remaining: Option<u64>,
}

impl Schedule {
    pub const fn new(interval: Duration, remaining: Option<u64>) -> Self {
        Self {
            interval,
            remaining,
        }
    }

    pub const fn interval(&self) -> Duration {
        self.interval
    }

    pub const fn is_finished(&self) -> bool {
        matches!(self.remaining, Some(0))
    }

    /// Deadline of the next tick after the one due at `previous` finished at `now`.
    ///
    /// An overdue tick runs as soon as the previous click is done. Earlier missed
    /// deadlines are dropped, and deadlines stay at least one interval apart.
    pub fn next_deadline(&self, previous: Instant, now: Instant) -> Instant {
        (previous + self.interval).max(now)
    }

    pub fn record_tick(&mut self) -> bool {
        if self.is_finished() {
            return false;
        }
        if let Some(remaining) = &mut self.remaining {
            *remaining = remaining.saturating_sub(1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_counter_stops_exactly() {
        let mut schedule = Schedule::new(Duration::from_millis(100), Some(3));
        assert!(schedule.record_tick());
        assert!(schedule.record_tick());
        assert!(schedule.record_tick());
        assert!(!schedule.record_tick());
        assert!(schedule.is_finished());
    }

    #[test]
    fn overdue_ticks_run_once_without_waiting_another_interval() {
        let interval = Duration::from_millis(10);
        let schedule = Schedule::new(interval, None);
        let start = Instant::now();
        // On time: keep the grid.
        assert_eq!(
            schedule.next_deadline(start, start + Duration::from_millis(4)),
            start + interval
        );
        // A click that ends after the next deadline is followed immediately.
        for late in [Duration::from_micros(500), Duration::from_millis(37)] {
            let now = start + interval + late;
            assert_eq!(schedule.next_deadline(start, now), now);
        }
    }

    #[test]
    fn overdue_ticks_never_exceed_the_configured_rate() {
        let interval = Duration::from_millis(10);
        let schedule = Schedule::new(interval, None);
        let mut deadline = Instant::now();
        // Clicks alternately run short and far beyond several intervals.
        for duration_ms in [2_u64, 45, 9, 11, 0, 30].iter().cycle().take(600) {
            let previous = deadline;
            let click_end = previous + Duration::from_millis(*duration_ms);
            deadline = schedule.next_deadline(previous, click_end);
            // No replayed burst: deadlines stay at least one interval apart.
            assert!(deadline >= previous + interval);
            assert!(deadline >= click_end);
        }
    }

    #[test]
    fn unlimited_schedule_keeps_running() {
        let mut schedule = Schedule::new(Duration::from_millis(10), None);
        for _ in 0..10 {
            assert!(schedule.record_tick());
            assert!(!schedule.is_finished());
        }
    }
}
