//! Watching folders for changes, and calming the results down.
//!
//! File systems are chatty: copying one file can fire a dozen events, and
//! copying a folder can fire thousands per second. A panel that reloaded on
//! every one of them would be unusable, so [`Coalescer`] collects a burst of
//! changes and says when it's worth looking at the folder again.

use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::VPath;

/// Called when a watched folder changed, from whichever thread the operating
/// system used. It gets the *folder* that was being watched, not the single
/// file that changed: for refreshing a panel that's all we need, and it means
/// every backend can report the same thing.
///
/// It runs on the watcher's thread, so it must not block. Sending the path
/// down a channel is the expected use.
pub type WatchSink = Arc<dyn Fn(VPath) + Send + Sync>;

/// Keeps a watch running. Dropping it stops the watch and releases whatever
/// the operating system was holding for it.
///
/// It's opaque on purpose: every backend keeps something different alive
/// (an inotify handle here, a polling task or an open connection later).
pub struct WatchHandle(#[allow(dead_code)] Box<dyn Any + Send + Sync>);

impl WatchHandle {
    /// Wraps whatever the backend must keep alive for the watch to work.
    pub fn new<T: Send + Sync + 'static>(guard: T) -> Self {
        WatchHandle(Box::new(guard))
    }
}

impl fmt::Debug for WatchHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WatchHandle")
    }
}

/// How long a folder must be quiet before a burst of changes counts as over.
pub const DEFAULT_SETTLE: Duration = Duration::from_millis(200);

/// How long we'll keep waiting for quiet before refreshing anyway, so that a
/// long-running copy still shows its progress in the panel.
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(1);

/// Turns a burst of file-system events into a few refreshes.
///
/// Changes are reported when the folder has been quiet for `settle`, or every
/// `max_wait` while events keep arriving, whichever comes first.
///
/// It has no clock of its own: the caller passes the current time in. That
/// keeps it predictable and lets the tests run instantly instead of sleeping.
#[derive(Debug, Clone)]
pub struct Coalescer {
    settle: Duration,
    max_wait: Duration,
    /// When the current burst started, if one is in progress.
    first: Option<Instant>,
    /// When the most recent change of the current burst arrived.
    last: Option<Instant>,
}

impl Coalescer {
    pub fn new(settle: Duration, max_wait: Duration) -> Self {
        Coalescer {
            settle,
            max_wait,
            first: None,
            last: None,
        }
    }

    /// Records that the folder changed.
    pub fn touch(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// Is a change waiting to be shown?
    pub fn is_pending(&self) -> bool {
        self.first.is_some()
    }

    /// Whether the folder should be reloaded now. Says yes at most once per
    /// burst; saying yes ends the burst.
    pub fn due(&mut self, now: Instant) -> bool {
        let (Some(first), Some(last)) = (self.first, self.last) else {
            return false;
        };
        // `duration_since` saturates at zero, so a clock that jumps backwards
        // just delays the refresh instead of panicking.
        let gone_quiet = now.duration_since(last) >= self.settle;
        let waited_long_enough = now.duration_since(first) >= self.max_wait;
        if gone_quiet || waited_long_enough {
            self.first = None;
            self.last = None;
            true
        } else {
            false
        }
    }
}

impl Default for Coalescer {
    fn default() -> Self {
        Coalescer::new(DEFAULT_SETTLE, DEFAULT_MAX_WAIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTLE: Duration = Duration::from_millis(200);
    const MAX_WAIT: Duration = Duration::from_secs(1);

    fn coalescer() -> Coalescer {
        Coalescer::new(SETTLE, MAX_WAIT)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_folder_that_never_changed_is_never_due() {
        let mut c = coalescer();
        let t0 = Instant::now();
        assert!(!c.is_pending());
        assert!(!c.due(t0));
        assert!(!c.due(t0 + MAX_WAIT * 10));
    }

    #[test]
    fn one_change_is_reported_once_the_folder_goes_quiet() {
        let mut c = coalescer();
        let t0 = Instant::now();
        c.touch(t0);

        assert!(c.is_pending());
        assert!(!c.due(t0 + ms(100)), "still inside the quiet period");
        assert!(c.due(t0 + SETTLE));
    }

    #[test]
    fn reporting_a_change_ends_the_burst() {
        let mut c = coalescer();
        let t0 = Instant::now();
        c.touch(t0);
        assert!(c.due(t0 + SETTLE));

        assert!(!c.is_pending());
        assert!(!c.due(t0 + SETTLE + MAX_WAIT * 10));
    }

    #[test]
    fn another_change_restarts_the_quiet_period() {
        let mut c = coalescer();
        let t0 = Instant::now();
        c.touch(t0);
        c.touch(t0 + ms(150));

        assert!(!c.due(t0 + ms(250)), "only 100ms since the last change");
        assert!(c.due(t0 + ms(350)));
    }

    #[test]
    fn a_steady_stream_of_changes_still_refreshes_every_max_wait() {
        let mut c = coalescer();
        let t0 = Instant::now();

        // A long copy: something changes every 50ms and the folder never goes quiet.
        let mut fired_at = None;
        for step in 0..40 {
            let now = t0 + ms(step * 50);
            c.touch(now);
            if c.due(now) {
                fired_at = Some(now);
                break;
            }
        }

        assert_eq!(
            fired_at,
            Some(t0 + MAX_WAIT),
            "should give up waiting for quiet after max_wait"
        );
    }

    #[test]
    fn the_max_wait_is_measured_from_the_start_of_the_burst() {
        let mut c = coalescer();
        let t0 = Instant::now();
        c.touch(t0);
        assert!(c.due(t0 + SETTLE));

        // A second burst gets its own full max_wait, not what's left of the first.
        c.touch(t0 + ms(500));
        assert!(!c.due(t0 + ms(600)));
        assert!(c.due(t0 + ms(500) + MAX_WAIT));
    }
}
