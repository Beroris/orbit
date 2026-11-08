//! The timer and the stopwatch. Both keep going with the launcher closed, and neither ticks in the
//! background: a running timer sets one Windows timer for the moment it ends, and the launcher
//! redraws once a second only while it's showing one of them. Times are taken from the wall clock,
//! so a timer still ends at the right moment after the PC has been asleep.

use std::time::{Duration, SystemTime};

fn now() -> SystemTime {
    SystemTime::now()
}

/// A timer counting down.
#[derive(Clone, Debug)]
pub struct Countdown {
    /// How long it was set for, in seconds, plus any time added.
    pub length: u64,
    /// When it ends, while it runs.
    ends: Option<SystemTime>,
    /// What was left when it was paused.
    left: Duration,
    pub done: bool,
}

impl Countdown {
    pub fn start(seconds: u64) -> Countdown {
        let length = Duration::from_secs(seconds);
        Countdown { length: seconds, ends: Some(now() + length), left: length, done: false }
    }

    /// A timer `length` seconds long with `left` of them still to go, for previews.
    pub fn with_left(length: u64, left: u64) -> Countdown {
        Countdown { length, ends: Some(now() + Duration::from_secs(left)), left: Duration::from_secs(left), done: false }
    }

    pub fn running(&self) -> bool {
        self.ends.is_some()
    }

    pub fn remaining(&self) -> Duration {
        match self.ends {
            Some(ends) => ends.duration_since(now()).unwrap_or(Duration::ZERO),
            None if self.done => Duration::ZERO,
            None => self.left,
        }
    }

    /// The seconds a clock shows: rounded up, so it reads 0:00 only once it has ended.
    pub fn shown_seconds(&self) -> u64 {
        let left = self.remaining();
        left.as_secs() + u64::from(left.subsec_nanos() > 0)
    }

    /// How much is left, from 1 when it starts to 0 when it ends.
    pub fn fraction_left(&self) -> f32 {
        if self.length == 0 { 0.0 } else { (self.remaining().as_secs_f32() / self.length as f32).clamp(0.0, 1.0) }
    }

    pub fn pause(&mut self) {
        if self.ends.is_some() {
            self.left = self.remaining();
            self.ends = None;
        }
    }

    pub fn resume(&mut self) {
        if self.ends.is_none() && !self.done {
            self.ends = Some(now() + self.left);
        }
    }

    pub fn add(&mut self, seconds: u64) {
        let more = Duration::from_secs(seconds);
        self.length += seconds;
        if self.done {
            self.done = false;
            self.ends = Some(now() + more);
        } else if let Some(ends) = self.ends {
            self.ends = Some(ends + more);
        } else {
            self.left += more;
        }
    }

    /// Marks it ended, if its time is up. True the once it ends.
    pub fn check_done(&mut self) -> bool {
        if self.running() && self.remaining().is_zero() {
            self.ends = None;
            self.done = true;
            return true;
        }
        false
    }
}

/// A stopwatch counting up.
#[derive(Clone, Debug, Default)]
pub struct Stopwatch {
    started: Option<SystemTime>,
    banked: Duration,
}

impl Stopwatch {
    pub fn running(&self) -> bool {
        self.started.is_some()
    }

    pub fn elapsed(&self) -> Duration {
        self.banked + self.started.map_or(Duration::ZERO, |started| now().duration_since(started).unwrap_or(Duration::ZERO))
    }

    /// A stopwatch that has been running for `seconds`, for previews.
    pub fn from_elapsed(seconds: u64) -> Stopwatch {
        Stopwatch { started: Some(now()), banked: Duration::from_secs(seconds) }
    }

    pub fn is_reset(&self) -> bool {
        !self.running() && self.banked.is_zero()
    }

    pub fn start(&mut self) {
        if self.started.is_none() {
            self.started = Some(now());
        }
    }

    pub fn pause(&mut self) {
        if self.started.is_some() {
            self.banked = self.elapsed();
            self.started = None;
        }
    }

    pub fn reset(&mut self) {
        *self = Stopwatch::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timer_pauses_resumes_and_takes_more_time() {
        let mut timer = Countdown::start(60);
        assert!(timer.running());
        assert!(timer.shown_seconds() <= 60 && timer.shown_seconds() >= 59);
        timer.pause();
        assert!(!timer.running());
        let left = timer.remaining();
        timer.add(60);
        assert_eq!(timer.remaining(), left + Duration::from_secs(60));
        assert_eq!(timer.length, 120);
        timer.resume();
        assert!(timer.running());
        assert!(!timer.check_done());
    }

    #[test]
    fn a_finished_timer_says_so_once() {
        let mut timer = Countdown { length: 1, ends: Some(now() - Duration::from_secs(1)), left: Duration::ZERO, done: false };
        assert!(timer.check_done());
        assert!(timer.done && !timer.running());
        assert!(!timer.check_done());
        assert_eq!(timer.shown_seconds(), 0);
        timer.add(30);
        assert!(timer.running() && !timer.done);
    }

    #[test]
    fn a_stopwatch_keeps_its_time_while_paused() {
        let mut watch = Stopwatch::default();
        assert!(watch.is_reset());
        watch.start();
        watch.pause();
        let held = watch.elapsed();
        assert_eq!(watch.elapsed(), held);
        watch.reset();
        assert!(watch.is_reset());
    }
}
