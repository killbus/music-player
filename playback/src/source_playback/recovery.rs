//! Host recovery budget. All clocks are supplied by the player tick for testing.
use crate::managed::EstimatedPosition;
use std::time::{Duration, Instant};

pub(super) const RETRY_LIMIT: u32 = 2;
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(45);
const STABLE_PLAYBACK: Duration = Duration::from_secs(10);
const PROGRESS_GAP: Duration = Duration::from_secs(2);

struct Progress {
    generation: u64,
    start: Duration,
    last: Duration,
    since: Instant,
    advanced_at: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed::StartPosition;
    use rockbox_playback::StreamOutputBoundary;

    fn position(seconds: u64) -> EstimatedPosition {
        EstimatedPosition {
            absolute: Duration::from_secs(seconds),
            requested_start: Duration::ZERO,
            start: StartPosition::RequestedOnly,
            boundary: StreamOutputBoundary::ByteStream,
            generation: 7,
        }
    }

    #[test]
    fn backoff_is_bounded_and_brief_progress_does_not_reset_budget() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        assert!(recovery.schedule(now, Duration::ZERO));
        assert_eq!(recovery.retry_at, Some(now + Duration::from_secs(1)));
        recovery.observe(position(1), now + Duration::from_secs(2));
        assert!(recovery.schedule(now + Duration::from_secs(3), Duration::from_secs(99)));
        assert_eq!(recovery.retry_at, Some(now + Duration::from_millis(6250)));
        recovery.observe(position(2), now + Duration::from_secs(7));
        assert!(!recovery.schedule(now + Duration::from_secs(8), Duration::ZERO));
        assert_eq!(recovery.attempt, 2);
        assert!(!recovery.active);
    }

    #[test]
    fn deadline_covers_resolution_and_opening_and_stalls_cannot_reset_it() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        assert!(recovery.schedule(now, Duration::ZERO));
        recovery.observe(position(1), now + Duration::from_secs(1));
        recovery.observe(position(1), now + Duration::from_secs(20));
        recovery.observe(position(20), now + Duration::from_secs(21));
        assert_eq!(recovery.attempt, 1);
        assert!(recovery.expired(now + Duration::from_secs(45)));
        assert!(!recovery.schedule(now + Duration::from_secs(45), Duration::ZERO));
    }

    #[test]
    fn only_sustained_delivered_pcm_replenishes_budget() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        assert!(recovery.schedule(now, Duration::ZERO));
        for second in 1..=10 {
            recovery.observe(position(second), now + Duration::from_secs(second));
        }
        assert_eq!(recovery.attempt, 1);
        recovery.observe(position(11), now + Duration::from_secs(11));
        assert_eq!(recovery.attempt, 0);
        assert!(recovery.schedule(now + Duration::from_secs(60), Duration::ZERO));
    }
}

#[derive(Default)]
pub(super) struct Recovery {
    pub attempt: u32,
    pub retry_at: Option<Instant>,
    deadline: Option<Instant>,
    progress: Option<Progress>,
    pub active: bool,
}

impl Recovery {
    pub fn schedule(&mut self, now: Instant, jitter: Duration) -> bool {
        self.progress = None;
        if self.attempt >= RETRY_LIMIT || self.expired(now) {
            self.finish();
            return false;
        }
        let deadline = *self.deadline.get_or_insert(now + RECOVERY_TIMEOUT);
        let delay = Duration::from_secs(if self.attempt == 0 { 1 } else { 3 })
            + jitter.min(Duration::from_millis(250));
        if now + delay >= deadline {
            self.finish();
            return false;
        }
        self.attempt += 1;
        self.retry_at = Some(now + delay);
        self.active = true;
        true
    }

    pub fn expired(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }

    pub fn finish(&mut self) {
        self.active = false;
        self.retry_at = None;
        self.progress = None;
    }

    /// A brief successful open must not permit endless failure/reopen loops.
    pub fn observe(&mut self, position: EstimatedPosition, now: Instant) {
        self.active = false;
        if self.attempt == 0 {
            return;
        }
        let progress = self.progress.get_or_insert(Progress {
            generation: position.generation,
            start: position.absolute,
            last: position.absolute,
            since: now,
            advanced_at: now,
        });
        if progress.generation != position.generation
            || position.absolute < progress.last
            || now.duration_since(progress.advanced_at) > PROGRESS_GAP
        {
            *progress = Progress {
                generation: position.generation,
                start: position.absolute,
                last: position.absolute,
                since: now,
                advanced_at: now,
            };
        }
        if position.absolute > progress.last {
            progress.last = position.absolute;
            progress.advanced_at = now;
        }
        if now.duration_since(progress.since) >= STABLE_PLAYBACK
            && position.absolute.saturating_sub(progress.start) >= STABLE_PLAYBACK
        {
            *self = Self::default();
        }
    }
}
