//! The HyperKey state machine.
//!
//! Host contract:
//! - Key codes are host-defined `u8`s (Windows VK codes, macOS virtual key codes).
//! - The host calls [`HyperMachine::on_key`] for every physical key event **except**
//!   events it injected itself. The call does no I/O and no allocation, so it is safe
//!   inside a `WH_KEYBOARD_LL` callback or a `CGEventTap`.
//! - The host applies the returned [`Decision`]: swallow or pass the event, then perform
//!   the effects in order.
//! - The host calls [`HyperMachine::reset`] on lock, sleep, suspend and shutdown, and
//!   performs the effects it returns so no virtual modifier can stay down.

/// What a tap of the trigger key (press and release with no chord) does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapBehavior {
    #[default]
    Escape,
    /// The original toggle behaviour of Caps Lock.
    Original,
    None,
    /// Press and release the trigger, then press a mapped key. Nothing is held down, so it needs
    /// no modifier handling at all. Any other key cancels it and goes through normally.
    Leader,
}

/// How long after a leader tap the next key still counts as its mapping.
pub const LEADER_WINDOW_MS: u32 = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HyperMode {
    /// Orbit interprets Hyper+key itself. Other apps never see a chord. Unbound keys are ignored.
    #[default]
    Internal,
    /// Unbound keys are forwarded with Ctrl+Alt+Shift+Super held, so other apps see a Hyper chord.
    Synthetic,
    /// No input hook at all. An external remapper produces the chord and Orbit registers it.
    External,
}

#[derive(Clone, Debug)]
pub struct HyperConfig {
    pub trigger: u8,
    pub mode: HyperMode,
    pub tap: TapBehavior,
    /// Longest hold that still counts as a tap, in milliseconds.
    pub tap_timeout_ms: u32,
}

/// Something the host must do, in order, after deciding about the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Run the action bound to this key code. Dispatch off the input thread.
    Dispatch { code: u8 },
    /// The trigger was tapped.
    Tap(TapBehavior),
    /// Press virtual Ctrl+Alt+Shift+Super, then press `code` with them held.
    SyntheticChordDown { code: u8 },
    /// Release the virtual modifiers. Includes a masking key so Super does not open a menu.
    SyntheticRelease,
}

/// Effects are bounded, so there is no allocation on the input path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub swallow: bool,
    effects: [Option<Effect>; 2],
}

impl Decision {
    pub const PASS: Decision = Decision { swallow: false, effects: [None, None] };
    pub const SWALLOW: Decision = Decision { swallow: true, effects: [None, None] };

    const fn swallow_with(effect: Effect) -> Decision {
        Decision { swallow: true, effects: [Some(effect), None] }
    }

    pub fn effects(&self) -> impl Iterator<Item = Effect> + '_ {
        self.effects.iter().flatten().copied()
    }
}

/// 256-bit set of key codes.
#[derive(Clone, Copy, Default)]
struct KeySet([u64; 4]);

impl KeySet {
    fn contains(&self, code: u8) -> bool {
        self.0[(code >> 6) as usize] & (1u64 << (code & 63)) != 0
    }
    fn insert(&mut self, code: u8) {
        self.0[(code >> 6) as usize] |= 1u64 << (code & 63);
    }
    fn remove(&mut self, code: u8) {
        self.0[(code >> 6) as usize] &= !(1u64 << (code & 63));
    }
    fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }
}

pub struct HyperMachine {
    config: HyperConfig,
    bound: KeySet,
    modifiers: KeySet,
    /// Every key the host has seen go down and not yet up, whether or not it was passed on.
    down: KeySet,
    /// Keys whose key-down was swallowed. Their key-up must be swallowed too, even if the
    /// trigger has already been released, or apps would see a stray key-up.
    swallowed: KeySet,
    /// Keys passed to the app under the virtual modifiers. They may outlive the trigger, and their
    /// autorepeat must not start typing bare letters once it has been released.
    forwarded: KeySet,
    held: bool,
    /// A chord happened during this hold, so the release is not a tap.
    chord_used: bool,
    /// Virtual modifiers are currently down in the OS.
    synthetic_active: bool,
    held_since_ms: u32,
    suspended: bool,
    /// A leader tap happened, and this is when it stops counting.
    leader_deadline_ms: Option<u32>,
    /// The host is asking which key to bind, so every key dispatches, bound or not.
    recording: bool,
}

impl HyperMachine {
    /// `modifier_codes` lists the host's codes for Ctrl, Alt, Shift and Super, left and right.
    /// They pass through during a hold and do not turn a tap into a chord.
    pub fn new(config: HyperConfig, modifier_codes: &[u8]) -> Self {
        let mut modifiers = KeySet::default();
        for &code in modifier_codes {
            modifiers.insert(code);
        }
        HyperMachine {
            config,
            bound: KeySet::default(),
            modifiers,
            down: KeySet::default(),
            swallowed: KeySet::default(),
            forwarded: KeySet::default(),
            held: false,
            chord_used: false,
            synthetic_active: false,
            held_since_ms: 0,
            suspended: false,
            leader_deadline_ms: None,
            recording: false,
        }
    }

    pub fn set_bound_keys(&mut self, codes: impl IntoIterator<Item = u8>) {
        self.bound = KeySet::default();
        for code in codes {
            self.bound.insert(code);
        }
    }

    /// While recording, Hyper plus any key dispatches that key, so the host can bind it.
    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
    }

    pub fn is_holding(&self) -> bool {
        self.held
    }

    /// True when the host may safely replace its input hook: no key is mid-chord.
    pub fn is_idle(&self) -> bool {
        !self.held && self.swallowed.is_empty() && !self.synthetic_active
    }

    /// While suspended every event passes untouched. Suspending during a hold finishes the
    /// hold safely: keys already swallowed still get their key-ups swallowed.
    pub fn set_suspended(&mut self, suspended: bool) -> Decision {
        self.suspended = suspended;
        // A key-up can be lost while suspended. A stale bit would make the next press look like
        // autorepeat, which would swallow it without starting a hold.
        self.down.remove(self.config.trigger);
        if suspended { self.end_hold() } else { Decision::PASS }
    }

    /// Forget all transient state. Returns the effects needed to leave the OS keyboard clean.
    pub fn reset(&mut self) -> Decision {
        let decision = self.end_hold();
        self.swallowed = KeySet::default();
        self.forwarded = KeySet::default();
        // Other keys may really still be down. Forgetting that would make the next autorepeat look
        // like a new press, so it could fire an action again, or swallow the key-up of a key the
        // app saw go down. If a key-up was lost instead, the cost is one ignored press in a chord.
        self.down.remove(self.config.trigger);
        decision
    }

    fn end_hold(&mut self) -> Decision {
        self.leader_deadline_ms = None;
        self.held = false;
        self.chord_used = false;
        if self.synthetic_active {
            self.synthetic_active = false;
            Decision { swallow: false, effects: [Some(Effect::SyntheticRelease), None] }
        } else {
            Decision::PASS
        }
    }

    /// Feed one physical event. `time_ms` is a wrapping millisecond clock from the OS event.
    #[inline]
    pub fn on_key(&mut self, code: u8, is_down: bool, time_ms: u32) -> Decision {
        let was_down = self.down.contains(code);
        if is_down {
            self.down.insert(code);
        } else {
            self.down.remove(code);
        }

        if self.swallowed.contains(code) {
            // Checked before everything else: the trigger may already be released while this
            // key is still down, and its autorepeat must not start typing.
            if !is_down {
                self.swallowed.remove(code);
            }
            return Decision::SWALLOW;
        }
        if self.forwarded.contains(code) {
            // The app saw this key go down, so it gets the key-up, but no more repeats.
            if is_down {
                return Decision::SWALLOW;
            }
            self.forwarded.remove(code);
            return Decision::PASS;
        }
        if self.suspended {
            return Decision::PASS;
        }

        if code == self.config.trigger {
            return self.on_trigger(is_down, was_down, time_ms);
        }
        if is_down && !self.modifiers.contains(code) {
            if let Some(deadline) = self.leader_deadline_ms.take() {
                let in_time = (time_ms.wrapping_sub(deadline) as i32) <= 0;
                if in_time && (self.recording || self.bound.contains(code)) {
                    self.swallowed.insert(code);
                    return Decision::swallow_with(Effect::Dispatch { code });
                }
            }
        }
        if !self.held || self.modifiers.contains(code) {
            return Decision::PASS;
        }
        if !is_down {
            // A key pressed before the trigger, released during the hold. The app saw its
            // key-down, so it must see the key-up.
            return Decision::PASS;
        }
        if was_down {
            // Autorepeat of a key that went down before the trigger. It is not a new press,
            // but it must not keep typing while Hyper is held. Its key-up still passes.
            return Decision::SWALLOW;
        }

        self.chord_used = true;
        if self.recording || self.bound.contains(code) {
            self.swallowed.insert(code);
            return Decision::swallow_with(Effect::Dispatch { code });
        }
        match self.config.mode {
            HyperMode::Synthetic => {
                self.forwarded.insert(code);
                if self.synthetic_active {
                    Decision::PASS // the virtual modifiers are already held
                } else {
                    // Swallow and re-inject, because an injected modifier queues behind the
                    // event being processed. The key-up then passes through unchanged.
                    self.synthetic_active = true;
                    Decision::swallow_with(Effect::SyntheticChordDown { code })
                }
            }
            HyperMode::Internal | HyperMode::External => {
                // Ignored. Without this the letter would be typed bare and surprise the user.
                self.swallowed.insert(code);
                Decision::SWALLOW
            }
        }
    }

    fn on_trigger(&mut self, is_down: bool, was_down: bool, time_ms: u32) -> Decision {
        if is_down {
            if !was_down && !self.held {
                self.leader_deadline_ms = None;
                self.held = true;
                self.chord_used = false;
                self.held_since_ms = time_ms;
            }
            return Decision::SWALLOW; // first press and autorepeat alike
        }
        if !self.held {
            return Decision::PASS; // key-up from before Orbit started or after a reset
        }
        let is_tap = !self.chord_used
            && time_ms.wrapping_sub(self.held_since_ms) <= self.config.tap_timeout_ms;
        let release = self.synthetic_active;
        self.held = false;
        self.chord_used = false;
        self.synthetic_active = false;
        if release {
            Decision::swallow_with(Effect::SyntheticRelease)
        } else if is_tap && self.config.tap == TapBehavior::Leader {
            self.leader_deadline_ms = Some(time_ms.wrapping_add(LEADER_WINDOW_MS));
            Decision::SWALLOW
        } else if is_tap && self.config.tap != TapBehavior::None {
            Decision::swallow_with(Effect::Tap(self.config.tap))
        } else {
            Decision::SWALLOW
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPS: u8 = 0x14;
    const LSHIFT: u8 = 0xA0;
    const C: u8 = b'C';
    const V: u8 = b'V';
    const X: u8 = b'X';

    #[test]
    fn recording_dispatches_any_key_once() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.set_recording(true);
        m.on_key(CAPS, true, 0);
        let d = m.on_key(X, true, 10);
        assert!(d.swallow);
        assert_eq!(d.effects().next(), Some(Effect::Dispatch { code: X }));
        // Autorepeat must not bind again, and the key-up stays hidden.
        assert_eq!(m.on_key(X, true, 40).effects().next(), None);
        assert!(m.on_key(X, false, 60).swallow);
        m.on_key(CAPS, false, 70);

        m.set_recording(false);
        m.on_key(CAPS, true, 100);
        assert_eq!(m.on_key(X, true, 110).effects().next(), None, "an unbound key is ignored again");
    }

    fn machine(mode: HyperMode, tap: TapBehavior) -> HyperMachine {
        let mut m = HyperMachine::new(
            HyperConfig { trigger: CAPS, mode, tap, tap_timeout_ms: 300 },
            &[LSHIFT, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C],
        );
        m.set_bound_keys([C, V]);
        m
    }

    fn effects(d: Decision) -> Vec<Effect> {
        d.effects().collect()
    }

    #[test]
    fn tap_fires_exactly_once() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        assert_eq!(m.on_key(CAPS, true, 0), Decision::SWALLOW);
        let up = m.on_key(CAPS, false, 50);
        assert!(up.swallow);
        assert_eq!(effects(up), vec![Effect::Tap(TapBehavior::Escape)]);
        assert!(m.is_idle());
    }

    #[test]
    fn slow_hold_without_chord_is_not_a_tap() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        let up = m.on_key(CAPS, false, 301);
        assert!(up.swallow);
        assert!(effects(up).is_empty());
    }

    #[test]
    fn tap_behaviour_none_does_nothing() {
        let mut m = machine(HyperMode::Internal, TapBehavior::None);
        m.on_key(CAPS, true, 0);
        assert!(effects(m.on_key(CAPS, false, 10)).is_empty());
    }

    #[test]
    fn hold_c_dispatches_once_and_leaves_no_stray_key() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        let down = m.on_key(C, true, 10);
        assert!(down.swallow);
        assert_eq!(effects(down), vec![Effect::Dispatch { code: C }]);
        // Autorepeat of both keys does nothing.
        for t in 11..40 {
            let repeat = m.on_key(C, true, t);
            assert!(repeat.swallow && effects(repeat).is_empty());
            let repeat = m.on_key(CAPS, true, t);
            assert!(repeat.swallow && effects(repeat).is_empty());
        }
        assert_eq!(m.on_key(C, false, 50), Decision::SWALLOW);
        let up = m.on_key(CAPS, false, 60);
        assert!(up.swallow);
        assert!(effects(up).is_empty(), "chord release must not fire the tap action");
        assert!(m.is_idle());
    }

    #[test]
    fn trigger_released_before_letter_still_swallows_letter_up() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        m.on_key(C, true, 5);
        let up = m.on_key(CAPS, false, 8);
        assert!(effects(up).is_empty());
        assert!(!m.is_idle(), "C is still physically down");
        assert_eq!(m.on_key(C, false, 12), Decision::SWALLOW);
        assert!(m.is_idle());
        // Normal typing resumes.
        assert_eq!(m.on_key(C, true, 20), Decision::PASS);
        assert_eq!(m.on_key(C, false, 30), Decision::PASS);
    }

    #[test]
    fn autorepeat_after_trigger_release_does_not_type() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        m.on_key(C, true, 5);
        m.on_key(CAPS, false, 8);
        for t in 9..30 {
            let repeat = m.on_key(C, true, t);
            assert!(repeat.swallow && effects(repeat).is_empty());
        }
        assert_eq!(m.on_key(C, false, 31), Decision::SWALLOW);
        assert_eq!(m.on_key(C, true, 40), Decision::PASS);
    }

    #[test]
    fn unbound_key_is_ignored_in_internal_mode() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        let down = m.on_key(X, true, 5);
        assert!(down.swallow && effects(down).is_empty());
        assert_eq!(m.on_key(X, false, 10), Decision::SWALLOW);
        assert!(effects(m.on_key(CAPS, false, 20)).is_empty());
    }

    #[test]
    fn modifiers_pass_and_do_not_cancel_a_tap() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        assert_eq!(m.on_key(LSHIFT, true, 5), Decision::PASS);
        assert_eq!(m.on_key(LSHIFT, false, 10), Decision::PASS);
        assert_eq!(effects(m.on_key(CAPS, false, 20)), vec![Effect::Tap(TapBehavior::Escape)]);
    }

    #[test]
    fn key_held_before_trigger_keeps_its_key_up() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        assert_eq!(m.on_key(X, true, 0), Decision::PASS);
        m.on_key(CAPS, true, 5);
        // Its autorepeat must not type during the hold and must not count as a chord.
        let repeat = m.on_key(X, true, 6);
        assert!(repeat.swallow && effects(repeat).is_empty());
        assert_eq!(m.on_key(X, false, 7), Decision::PASS, "the app saw the key-down");
        assert_eq!(effects(m.on_key(CAPS, false, 8)), vec![Effect::Tap(TapBehavior::Escape)]);
    }

    #[test]
    fn multiple_chords_in_one_hold() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        assert_eq!(effects(m.on_key(C, true, 1)), vec![Effect::Dispatch { code: C }]);
        m.on_key(C, false, 2);
        assert_eq!(effects(m.on_key(V, true, 3)), vec![Effect::Dispatch { code: V }]);
        m.on_key(V, false, 4);
        assert_eq!(effects(m.on_key(C, true, 5)), vec![Effect::Dispatch { code: C }]);
        m.on_key(C, false, 6);
        assert!(effects(m.on_key(CAPS, false, 7)).is_empty());
    }

    #[test]
    fn synthetic_mode_forwards_unbound_and_releases() {
        let mut m = machine(HyperMode::Synthetic, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        let first = m.on_key(X, true, 1);
        assert!(first.swallow);
        assert_eq!(effects(first), vec![Effect::SyntheticChordDown { code: X }]);
        assert_eq!(m.on_key(X, false, 2), Decision::PASS, "real key-up matches injected key-down");
        // The next key runs under the modifiers already held.
        assert_eq!(m.on_key(b'Y', true, 3), Decision::PASS);
        assert_eq!(m.on_key(b'Y', false, 4), Decision::PASS);
        // Bound keys are still Orbit's.
        assert_eq!(effects(m.on_key(C, true, 5)), vec![Effect::Dispatch { code: C }]);
        m.on_key(C, false, 6);
        assert_eq!(effects(m.on_key(CAPS, false, 7)), vec![Effect::SyntheticRelease]);
        assert!(m.is_idle());
    }

    #[test]
    fn reset_mid_chord_releases_modifiers_and_clears_state() {
        let mut m = machine(HyperMode::Synthetic, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        m.on_key(X, true, 1);
        assert_eq!(effects(m.reset()), vec![Effect::SyntheticRelease]);
        assert!(m.is_idle());
        // The key-up that arrives after waking does not fire anything.
        assert_eq!(m.on_key(CAPS, false, 500), Decision::PASS);
        assert_eq!(m.on_key(X, false, 501), Decision::PASS);
        // And a second reset has nothing left to release.
        assert!(effects(m.reset()).is_empty());
    }

    #[test]
    fn suspend_passes_everything_but_finishes_held_keys() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        m.on_key(C, true, 1);
        assert!(effects(m.set_suspended(true)).is_empty());
        assert_eq!(m.on_key(C, false, 2), Decision::SWALLOW, "its key-down was swallowed");
        assert_eq!(m.on_key(CAPS, false, 3), Decision::PASS);
        assert_eq!(m.on_key(CAPS, true, 4), Decision::PASS);
        assert_eq!(m.on_key(C, true, 5), Decision::PASS);
        m.set_suspended(false);
    }

    #[test]
    fn synthetic_chord_key_does_not_repeat_bare_after_release() {
        let mut m = machine(HyperMode::Synthetic, TapBehavior::Escape);
        m.on_key(CAPS, true, 0);
        assert_eq!(effects(m.on_key(X, true, 1)), vec![Effect::SyntheticChordDown { code: X }]);
        assert_eq!(m.on_key(b'Y', true, 2), Decision::PASS);
        assert_eq!(effects(m.on_key(CAPS, false, 3)), vec![Effect::SyntheticRelease]);
        // Both keys are still down, and neither may type now that the modifiers are gone.
        for t in 4..20 {
            assert_eq!(m.on_key(X, true, t), Decision::SWALLOW);
            assert_eq!(m.on_key(b'Y', true, t), Decision::SWALLOW);
        }
        assert_eq!(m.on_key(X, false, 21), Decision::PASS);
        assert_eq!(m.on_key(b'Y', false, 22), Decision::PASS);
        // And the next press is ordinary typing.
        assert_eq!(m.on_key(X, true, 30), Decision::PASS);
        assert_eq!(m.on_key(X, false, 31), Decision::PASS);
    }

    #[test]
    fn lost_key_up_while_suspended_does_not_eat_the_next_hold() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.set_suspended(true);
        assert_eq!(m.on_key(CAPS, true, 0), Decision::PASS);
        // Its key-up never arrives.
        m.set_suspended(false);
        assert_eq!(m.on_key(CAPS, true, 100), Decision::SWALLOW);
        assert!(m.is_holding(), "the press must start a hold");
        assert_eq!(effects(m.on_key(C, true, 101)), vec![Effect::Dispatch { code: C }]);
    }

    #[test]
    fn leader_tap_then_mapped_key_dispatches_once() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Leader);
        assert_eq!(m.on_key(CAPS, true, 0), Decision::SWALLOW);
        assert_eq!(m.on_key(CAPS, false, 50), Decision::SWALLOW);
        assert!(m.is_idle(), "nothing is held during the wait");
        assert_eq!(effects(m.on_key(C, true, 200)), vec![Effect::Dispatch { code: C }]);
        assert_eq!(m.on_key(C, true, 210), Decision::SWALLOW, "its repeat does nothing");
        assert_eq!(m.on_key(C, false, 220), Decision::SWALLOW);
        assert_eq!(m.on_key(C, true, 300), Decision::PASS, "the next C is typing");
    }

    #[test]
    fn leader_is_cancelled_by_an_unmapped_key_or_by_time() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Leader);
        m.on_key(CAPS, true, 0);
        m.on_key(CAPS, false, 50);
        assert_eq!(m.on_key(X, true, 100), Decision::PASS, "an unmapped key still types");
        m.on_key(X, false, 110);
        assert_eq!(m.on_key(C, true, 120), Decision::PASS, "one key only");
        m.on_key(C, false, 130);

        m.on_key(CAPS, true, 200);
        m.on_key(CAPS, false, 250);
        assert_eq!(m.on_key(C, true, 250 + LEADER_WINDOW_MS + 1), Decision::PASS, "too late");
    }

    #[test]
    fn leader_ignores_modifiers_and_a_reset_cancels_it() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Leader);
        m.on_key(CAPS, true, 0);
        m.on_key(CAPS, false, 50);
        assert_eq!(m.on_key(LSHIFT, true, 60), Decision::PASS);
        m.on_key(LSHIFT, false, 70);
        assert_eq!(effects(m.on_key(C, true, 80)), vec![Effect::Dispatch { code: C }]);
        m.on_key(C, false, 90);

        m.on_key(CAPS, true, 100);
        m.on_key(CAPS, false, 150);
        let _ = m.reset();
        assert_eq!(m.on_key(C, true, 160), Decision::PASS);
    }

    #[test]
    fn clock_wraparound_still_detects_tap() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        m.on_key(CAPS, true, u32::MAX - 10);
        assert_eq!(effects(m.on_key(CAPS, false, 20)), vec![Effect::Tap(TapBehavior::Escape)]);
    }

    #[test]
    fn typing_is_untouched_without_trigger() {
        let mut m = machine(HyperMode::Internal, TapBehavior::Escape);
        for code in 0x41..=0x5A {
            assert_eq!(m.on_key(code, true, 0), Decision::PASS);
            assert_eq!(m.on_key(code, false, 1), Decision::PASS);
        }
    }

    /// Randomised sequences: whatever the order, no key-up may be swallowed without its key-down
    /// also having been swallowed, and every swallowed key-down gets a swallowed key-up.
    #[test]
    fn swallow_is_always_balanced() {
        let keys = [CAPS, C, V, X, LSHIFT];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        for mode in [HyperMode::Internal, HyperMode::Synthetic] {
            for _ in 0..2_000 {
                let mut m = machine(mode, TapBehavior::Escape);
                let mut phys = [false; 5];
                let mut app_sees_down = [false; 5];
                for t in 0..64u32 {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    if seed % 97 == 0 {
                        // After a reset the machine has forgotten everything, so the app's view
                        // is only tracked again from the next key-down.
                        let _ = m.reset();
                        app_sees_down = phys;
                    } else if seed % 89 == 0 {
                        let _ = m.set_suspended(seed & 0x200 != 0);
                    }
                    let i = (seed % 5) as usize;
                    // Bias to key-downs while up, and allow repeated key-downs for autorepeat.
                    let is_down = !phys[i] || seed & 0x100 != 0;
                    let is_repeat = is_down && phys[i];
                    phys[i] = is_down;
                    let d = m.on_key(keys[i], is_down, t * 7);
                    let injects = d
                        .effects()
                        .any(|e| matches!(e, Effect::SyntheticChordDown { code } if code == keys[i]));
                    if is_down {
                        if !d.swallow || injects {
                            app_sees_down[i] = true;
                        }
                    } else {
                        if d.swallow {
                            assert!(
                                !app_sees_down[i] || keys[i] == CAPS,
                                "swallowed key-up of {:#x} the app saw go down (repeat={is_repeat})",
                                keys[i]
                            );
                        }
                        app_sees_down[i] = false;
                    }
                }
                // Release everything. The machine must end idle with nothing virtual held.
                let _ = m.set_suspended(false);
                for (i, down) in phys.iter().enumerate() {
                    if *down {
                        m.on_key(keys[i], false, 10_000);
                    }
                }
                assert!(m.is_idle(), "machine not idle after all keys released");
            }
        }
    }
}
