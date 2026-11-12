//! The launcher's motion: it fades in and out, the selection glides from row to row, a menu rises
//! into place, one page gives way to the next, and the bottom edge follows the results.
//!
//! None of it holds anything up. What's shown changes underneath at once, keys and clicks act on
//! it straight away, and only the picture catches up, in a tenth of a second or so. Typing never
//! waits on a frame.
//!
//! Frames come from a small thread, `orbit-motion`, that waits for the screen's next refresh with
//! `DwmFlush` and posts [`WM_ORBIT_TICK`]. It sleeps while nothing moves, and never leaves more than
//! one tick waiting, so the launcher's own messages always get their turn between frames.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread::Thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW, WM_APP,
};

/// Time for the next frame.
pub const WM_ORBIT_TICK: u32 = WM_APP + 55;

/// The launcher appearing: a fade, as it rises the last few pixels into place.
pub const SHOW: Duration = Duration::from_millis(150);
/// How far below its place it starts, in DIPs.
pub const SHOW_RISE_DIP: f32 = 8.0;
/// How much of the launcher shows on the very first frame, so it's there the moment it's asked for
/// and only settles in from there.
pub const SHOW_FROM: f32 = 0.3;
/// The launcher going away, quicker than it came.
pub const HIDE: Duration = Duration::from_millis(110);
/// The selection gliding to another row.
pub const GLIDE: Duration = Duration::from_millis(110);
/// A menu rising from the footer, or dropping from its button.
pub const MENU: Duration = Duration::from_millis(140);
/// One page giving way to another, or a menu fading as it closes. The old one clears quickly, so
/// two pages' words are never muddled together for more than a frame or two.
pub const CROSSFADE: Duration = Duration::from_millis(100);
/// The bottom edge following the results.
pub const RESIZE: Duration = Duration::from_millis(150);

/// The fastest frames are drawn, so a very fast screen doesn't cost more than it shows.
const FRAME_MIN: Duration = Duration::from_micros(6_900);

/// Whether Windows is set to show animations, from Settings → Accessibility → Visual effects.
pub fn windows_animates() -> bool {
    let mut on = windows::core::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

/// Fast at first, settling gently: what things moving into place should do.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A touch more decisive than [`ease_out`], for small moves that should feel snappy.
pub fn ease_out_quart(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(4)
}

/// How far along a motion that started at `start` and lasts `length` is, from 0 to 1.
pub fn progress(start: Instant, length: Duration, now: Instant) -> f32 {
    if length.is_zero() {
        return 1.0;
    }
    (now.saturating_duration_since(start).as_secs_f32() / length.as_secs_f32()).clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A rectangle in fractions of a pixel, so a glide moves smoothly rather than a pixel at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rectf {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rectf {
    pub fn of(rect: RECT) -> Rectf {
        Rectf { left: rect.left as f32, top: rect.top as f32, right: rect.right as f32, bottom: rect.bottom as f32 }
    }

    pub fn to_rect(self) -> RECT {
        RECT { left: self.left.round() as i32, top: self.top.round() as i32, right: self.right.round() as i32, bottom: self.bottom.round() as i32 }
    }

    fn lerp(self, to: Rectf, t: f32) -> Rectf {
        Rectf { left: lerp(self.left, to.left, t), top: lerp(self.top, to.top, t), right: lerp(self.right, to.right, t), bottom: lerp(self.bottom, to.bottom, t) }
    }
}

/// Something that slides from where it was to where it's wanted: a highlight from row to row. Asked
/// each frame where its target is, it starts again from wherever it's got to whenever the target
/// moves, so a held arrow key or a quick pointer never leaves it behind or makes it jump.
#[derive(Default)]
pub struct Glide {
    /// What the target belongs to. A new list, a new query or a scroll means a new one, and the
    /// highlight jumps there rather than sliding across content that's changed under it.
    key: u64,
    from: Option<Rectf>,
    to: Option<Rectf>,
    start: Option<Instant>,
}

impl Glide {
    /// Where to draw it this frame, for a target in this context.
    pub fn aim(&mut self, key: u64, target: RECT, now: Instant, animate: bool) -> Rectf {
        let target = Rectf::of(target);
        if !animate || self.key != key || self.to.is_none() {
            *self = Glide { key, from: Some(target), to: Some(target), start: None };
            return target;
        }
        if self.to != Some(target) {
            let here = self.at(now);
            self.from = Some(here);
            self.to = Some(target);
            self.start = Some(now);
        }
        self.at(now)
    }

    fn at(&self, now: Instant) -> Rectf {
        let (Some(from), Some(to)) = (self.from, self.to) else { return Rectf { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 } };
        match self.start {
            Some(start) => from.lerp(to, ease_out_quart(progress(start, GLIDE, now))),
            None => to,
        }
    }

    pub fn moving(&self, now: Instant) -> bool {
        self.start.is_some_and(|start| progress(start, GLIDE, now) < 1.0)
    }

    /// Forgets where it was, so the next target is taken at once.
    pub fn reset(&mut self) {
        *self = Glide::default();
    }
}

/// A number that eases to wherever it's last been asked to go, such as the launcher's height.
#[derive(Default)]
pub struct Ease {
    from: f32,
    to: f32,
    start: Option<Instant>,
    set: bool,
}

impl Ease {
    pub fn aim(&mut self, target: f32, length: Duration, now: Instant, animate: bool) -> f32 {
        if !animate || !self.set {
            *self = Ease { from: target, to: target, start: None, set: true };
            return target;
        }
        if self.to != target {
            self.from = self.at(length, now);
            self.to = target;
            self.start = Some(now);
        }
        self.at(length, now)
    }

    fn at(&self, length: Duration, now: Instant) -> f32 {
        match self.start {
            Some(start) => lerp(self.from, self.to, ease_out(progress(start, length, now))),
            None => self.to,
        }
    }

    pub fn moving(&self, length: Duration, now: Instant) -> bool {
        self.start.is_some_and(|start| progress(start, length, now) < 1.0)
    }

    pub fn reset(&mut self) {
        *self = Ease::default();
    }
}

/// The last frame of one page, fading out over the first frames of the next.
pub struct Crossfade {
    pub pixels: Vec<u32>,
    pub width: i32,
    pub height: i32,
    pub start: Instant,
}

impl Crossfade {
    /// How much of the old picture still shows, from 1 to 0.
    pub fn remaining(&self, now: Instant) -> f32 {
        1.0 - ease_out_quart(progress(self.start, CROSSFADE, now))
    }
}

/// The window coming or going, which only changes how see-through it is and where it sits, so
/// none of its frames needs drawing again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fade {
    /// `x` and `y` are where it ends up. `from` is how opaque it starts, and `rise` whether it
    /// comes up from a little below: not when it's coming back before it had quite gone.
    In { start: Instant, x: i32, y: i32, from: f32, rise: bool },
    /// `from` is how opaque it was when it began to go, in case it hadn't finished coming.
    Out { start: Instant, from: f32 },
}

impl Fade {
    /// How opaque the window is, from 0 to 1, and how far below its place it is, from 0 to 1 of the rise.
    pub fn at(&self, now: Instant) -> (f32, f32) {
        match *self {
            Fade::In { start, from, rise, .. } => {
                let t = progress(start, SHOW, now);
                // Opacity comes a little ahead of position, so it's there before it settles.
                let opacity = from + (1.0 - from) * ease_out(t * 1.25);
                (opacity, if rise { 1.0 - ease_out(t) } else { 0.0 })
            }
            Fade::Out { start, from } => (from * (1.0 - ease_out(progress(start, HIDE, now))), 0.0),
        }
    }

    pub fn done(&self, now: Instant) -> bool {
        match *self {
            Fade::In { start, .. } => progress(start, SHOW, now) >= 1.0,
            Fade::Out { start, .. } => progress(start, HIDE, now) >= 1.0,
        }
    }
}

/// Everything in the launcher that's on its way somewhere.
#[derive(Default)]
pub struct Motion {
    pub selection: Glide,
    pub menu_selection: Glide,
    /// Which menu is showing, and when it began to appear, if it's still coming.
    pub menu_open: Option<(u64, Option<Instant>)>,
    /// The launcher's height, in pixels.
    pub height: Ease,
    pub crossfade: Option<Crossfade>,
    /// What page was drawn last, to tell when another has replaced it.
    pub page: Option<u64>,
    pub fade: Option<Fade>,
    /// The last frame drawn wasn't the final one: something was still on its way.
    pub unsettled: bool,
}

impl Motion {
    /// Whether anything drawn inside the launcher is still on its way.
    pub fn moving(&self, now: Instant) -> bool {
        self.selection.moving(now)
            || self.menu_selection.moving(now)
            || self.menu_open.is_some_and(|(_, start)| start.is_some_and(|start| progress(start, MENU, now) < 1.0))
            || self.height.moving(RESIZE, now)
            || self.crossfade.as_ref().is_some_and(|fade| fade.remaining(now) > 0.0)
    }

    /// Whether frames are still needed: to draw again, or to fade the window.
    pub fn busy(&self) -> bool {
        self.unsettled || self.fade.is_some()
    }

    /// How far a menu has come in, from 0 to 1.
    pub fn menu_in(&self, now: Instant) -> f32 {
        match self.menu_open {
            Some((_, Some(start))) => ease_out(progress(start, MENU, now)),
            _ => 1.0,
        }
    }

    /// Everything to where it's going, at once, as when the launcher opens.
    pub fn settle(&mut self) {
        self.selection.reset();
        self.menu_selection.reset();
        self.menu_open = None;
        self.height.reset();
        self.crossfade = None;
        self.page = None;
        self.unsettled = false;
    }
}

/// A key for a glide or a page, from the things that make it what it is, such as a tuple of them.
pub fn key(parts: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

// The frame clock.

/// The window ticks go to.
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// Frames are wanted.
static WANTED: AtomicBool = AtomicBool::new(false);
/// A tick has been posted and not yet handled.
static PENDING: AtomicBool = AtomicBool::new(false);

fn clock() -> &'static Thread {
    static CLOCK: OnceLock<Thread> = OnceLock::new();
    CLOCK.get_or_init(|| {
        std::thread::Builder::new()
            .name("orbit-motion".into())
            .spawn(run_clock)
            .map(|handle| handle.thread().clone())
            .unwrap_or_else(|_| std::thread::current())
    })
}

/// Asks for frames to be sent to `window` until [`stop`] is called.
pub fn want_frames(window: HWND) {
    TARGET.store(window.0 as isize, Ordering::Relaxed);
    if !WANTED.swap(true, Ordering::AcqRel) {
        clock().unpark();
    }
}

/// No more frames are needed.
pub fn stop() {
    WANTED.store(false, Ordering::Release);
}

/// A tick has been handled, so the next can be sent.
pub fn tick_done() {
    PENDING.store(false, Ordering::Release);
}

fn run_clock() {
    let mut last = Instant::now();
    loop {
        while !WANTED.load(Ordering::Acquire) {
            std::thread::park();
        }
        // The next refresh, or the one after on a screen faster than frames are worth drawing.
        loop {
            let before = Instant::now();
            let flushed = unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_ok();
            let now = Instant::now();
            if now.duration_since(last) >= FRAME_MIN {
                break;
            }
            // Nothing being drawn can make the compositor answer at once. Don't spin on it.
            if !flushed || now.duration_since(before) < Duration::from_micros(500) {
                std::thread::sleep(FRAME_MIN.saturating_sub(now.duration_since(last)));
                break;
            }
        }
        last = Instant::now();
        if !WANTED.load(Ordering::Acquire) || PENDING.swap(true, Ordering::AcqRel) {
            continue;
        }
        let window = HWND(TARGET.load(Ordering::Relaxed) as *mut _);
        if unsafe { PostMessageW(Some(window), WM_ORBIT_TICK, WPARAM(0), LPARAM(0)) }.is_err() {
            PENDING.store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(top: i32) -> RECT {
        RECT { left: 10, top, right: 300, bottom: top + 44 }
    }

    #[test]
    fn a_glide_starts_where_it_was_and_ends_on_the_target() {
        let mut glide = Glide::default();
        let start = Instant::now();
        assert_eq!(glide.aim(1, rect(0), start, true).to_rect(), rect(0), "the first target is taken at once");
        let first = glide.aim(1, rect(44), start, true);
        assert_eq!(first.to_rect(), rect(0), "it starts where it was");
        let halfway = glide.aim(1, rect(44), start + GLIDE / 2, true);
        assert!(halfway.top > 22.0 && halfway.top < 44.0, "eased: well past half way at half time, {}", halfway.top);
        assert_eq!(glide.aim(1, rect(44), start + GLIDE, true).to_rect(), rect(44));
        assert!(!glide.moving(start + GLIDE));
    }

    #[test]
    fn a_glide_turns_from_wherever_it_has_got_to() {
        let mut glide = Glide::default();
        let start = Instant::now();
        glide.aim(1, rect(0), start, true);
        glide.aim(1, rect(88), start, true);
        let partway = glide.aim(1, rect(88), start + GLIDE / 4, true);
        // A new target halfway there sets off from the highlight, not from either row.
        let turned = glide.aim(1, rect(0), start + GLIDE / 4, true);
        assert_eq!(turned, partway);
        assert!(glide.moving(start + GLIDE / 4));
    }

    #[test]
    fn a_new_list_or_no_motion_jumps() {
        let mut glide = Glide::default();
        let start = Instant::now();
        glide.aim(1, rect(0), start, true);
        assert_eq!(glide.aim(2, rect(132), start, true).to_rect(), rect(132), "another list");
        assert_eq!(glide.aim(2, rect(0), start, false).to_rect(), rect(0), "animations off");
        assert!(!glide.moving(start));
    }

    #[test]
    fn an_ease_follows_its_target_and_settles() {
        let mut height = Ease::default();
        let start = Instant::now();
        assert_eq!(height.aim(500.0, RESIZE, start, true), 500.0);
        assert_eq!(height.aim(200.0, RESIZE, start, true), 500.0);
        let middle = height.aim(200.0, RESIZE, start + RESIZE / 2, true);
        assert!(middle < 350.0 && middle > 200.0, "{middle}");
        assert_eq!(height.aim(200.0, RESIZE, start + RESIZE, true), 200.0);
        assert!(!height.moving(RESIZE, start + RESIZE));
    }

    #[test]
    fn the_window_is_there_before_it_has_settled() {
        let start = Instant::now();
        let fade = Fade::In { start, x: 0, y: 0, from: SHOW_FROM, rise: true };
        let (opacity, below) = fade.at(start + SHOW * 3 / 5);
        assert!(opacity > 0.97 && below > 0.0, "{opacity} {below}");
        assert!(fade.at(start).0 >= SHOW_FROM, "it shows at once");
        assert_eq!(fade.at(start + SHOW), (1.0, 0.0));
        assert!(Fade::Out { start, from: 1.0 }.at(start + HIDE).0 <= 0.0);
    }

    #[test]
    fn coming_back_while_it_fades_away_carries_on_from_there() {
        let start = Instant::now();
        let going = Fade::Out { start, from: 1.0 };
        let (left, _) = going.at(start + HIDE / 3);
        assert!(left > 0.0 && left < 1.0, "{left}");
        // It doesn't jump back to the start of its fade, or drop below where it was.
        let coming = Fade::In { start: start + HIDE / 3, x: 0, y: 0, from: left, rise: false };
        assert_eq!(coming.at(start + HIDE / 3), (left, 0.0));
        assert!(coming.at(start + HIDE / 3 + SHOW / 4).0 > left);
        // Going again before it had fully come starts from how much showed.
        let (partly, _) = Fade::In { start, x: 0, y: 0, from: SHOW_FROM, rise: true }.at(start + SHOW / 5);
        assert!(Fade::Out { start, from: partly }.at(start).0 <= partly + f32::EPSILON);
    }

    #[test]
    fn an_old_page_is_mostly_gone_within_two_frames() {
        let start = Instant::now();
        let fade = Crossfade { pixels: Vec::new(), width: 0, height: 0, start };
        assert_eq!(fade.remaining(start), 1.0);
        assert!(fade.remaining(start + Duration::from_millis(33)) < 0.25, "{}", fade.remaining(start + Duration::from_millis(33)));
        assert_eq!(fade.remaining(start + CROSSFADE), 0.0);
    }

    #[test]
    fn nothing_is_busy_once_everything_has_arrived() {
        let mut motion = Motion::default();
        let start = Instant::now();
        motion.selection.aim(1, rect(0), start, true);
        motion.selection.aim(1, rect(44), start, true);
        motion.menu_open = Some((1, Some(start)));
        assert!(motion.moving(start));
        assert!(!motion.moving(start + Duration::from_secs(1)));
    }
}
