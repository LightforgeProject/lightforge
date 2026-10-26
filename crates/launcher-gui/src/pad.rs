//! Gamepads, read straight from the kernel.
//!
//! The selection ring was first driven by keys alone, on the theory that
//! Steam Input turns a Deck's D-pad into arrows. It does that only under a
//! keyboard layout. Under the gamepad layout Steam gives a non-Steam game by
//! default in Game Mode, the game sees a virtual Xbox pad and no keys at all,
//! so the launcher sat still while Steam's own menus worked. Reading the pad
//! here makes the ring move whatever layout Steam chose.
//!
//! The D-pad and the left stick step the ring, A presses, B goes back. A
//! held direction repeats, the way a held arrow key does.
//!
//! Steam names the physical pads it is already translating in
//! `SDL_GAMECONTROLLER_IGNORE_DEVICES` when it launches a game, so the same
//! press does not arrive twice, once raw and once through Steam's virtual
//! pad. Those pads are skipped here too. A launcher started some other way
//! while Steam runs gets no such list and sees both, so beyond that one pad
//! has the floor while presses keep coming and the same press from another
//! is dropped.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use futures_util::Stream;
use gilrs::{Axis, Button, EventType, Gilrs};

/// A pad, by gilrs's number for it.
type PadId = usize;

use crate::focus::Step;

/// What a pad asks the ring to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Step(Step),
    Press,
    Back,
}

/// How long a direction is held before it starts repeating, and how often
/// it repeats after that. Close to a desktop's key repeat, a little slower,
/// because a ring that runs past its target on a list is worse than one
/// that takes a moment longer to get there.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_EVERY: Duration = Duration::from_millis(130);

/// The stick counts as pushed past this, and as let go under the second.
/// The gap keeps a stick resting near the edge from stuttering.
const STICK_ON: f32 = 0.6;
const STICK_OFF: f32 = 0.4;

/// How long the pad that pressed last keeps the floor. Measured 2026-10-17
/// with Steam running on the desktop: every press arrived from the Turtle
/// Beach and from Steam's virtual `28de:11ff` pad, so each step went two
/// controls. Longer than the repeat, so a held direction keeps it.
const FLOOR: Duration = Duration::from_millis(150);

/// Every press from every pad, for as long as the launcher runs. A machine
/// with no pads, or one where the input devices cannot be opened, gives an
/// empty stream and nothing else changes.
pub fn inputs() -> impl Stream<Item = Input> {
    iced::stream::channel(16, async |mut output| {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        // gilrs blocks while it waits, so it gets a thread of its own.
        let spawned = std::thread::Builder::new()
            .name("gamepads".to_owned())
            .spawn(move || read(send));
        if spawned.is_err() {
            return;
        }
        while let Some(input) = receive.recv().await {
            use futures_util::SinkExt;
            if output.send(input).await.is_err() {
                return;
            }
        }
    })
}

fn read(send: tokio::sync::mpsc::UnboundedSender<Input>) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(e) => {
            tracing::warn!("gamepads unavailable: {e}");
            return;
        }
    };
    let ignored = Ignored::from_env();
    let mut held = Held::default();
    let mut floor = Floor::default();
    loop {
        let wait = held.next.map(|at| at.saturating_duration_since(Instant::now()));
        let event = gilrs.next_event_blocking(wait);
        let now = Instant::now();
        let Some(event) = event else {
            // Not only a timeout: gilrs also comes back empty for an event
            // its filters dropped, which a resting stick's noise makes every
            // few milliseconds. So the hold is asked whether it is due.
            if let Some((pad, step)) = held.repeat(now) {
                floor.take(pad, now);
                crate::trace::pad(format_args!("pad {pad} -> {:?} (held)", Input::Step(step)));
                if send.send(Input::Step(step)).is_err() {
                    return;
                }
            }
            continue;
        };
        let id = PadId::from(event.id);
        let pad = gilrs.gamepad(event.id);
        let skipped = ignored.skips(pad.vendor_id(), pad.product_id());
        if !matches!(event.event, EventType::AxisChanged(..) | EventType::ButtonChanged(..)) {
            crate::trace::pad(format_args!(
                "pad {id} {:04x?}:{:04x?} {:?}{}",
                pad.vendor_id(),
                pad.product_id(),
                event.event,
                if skipped { " (skipped: Steam's list)" } else { "" },
            ));
        }
        if skipped {
            continue;
        }
        // Releases and the stick's resting state are kept for every pad, so
        // one that loses the floor mid-push is not out of step when it gets
        // it back; only what would move the ring waits for the floor.
        let input = match event.event {
            EventType::ButtonPressed(button, _) => match button {
                Button::South => Some(Input::Press),
                Button::East => Some(Input::Back),
                _ => dpad(button).map(|step| (Source::Dpad, step)).and_then(|(source, step)| {
                    floor.take(id, now).then(|| held.push(id, source, step, now))
                }),
            },
            EventType::ButtonReleased(button, _) => {
                if let Some(step) = dpad(button) {
                    held.release(id, Source::Dpad, step);
                }
                None
            }
            EventType::AxisChanged(axis, value, _) => match held.stick(id, axis, value) {
                Some(step) if floor.take(id, now) => Some(held.push(id, Source::Stick, step, now)),
                _ => None,
            },
            EventType::Disconnected => {
                held.forget(id);
                None
            }
            _ => None,
        };
        let input = match input {
            Some(Input::Press | Input::Back) if !floor.take(id, now) => None,
            input => input,
        };
        if let Some(input) = input {
            crate::trace::pad(format_args!("pad {id} -> {input:?}"));
            if send.send(input).is_err() {
                return;
            }
        }
    }
}

fn dpad(button: Button) -> Option<Step> {
    match button {
        Button::DPadUp => Some(Step::Up),
        Button::DPadDown => Some(Step::Down),
        Button::DPadLeft => Some(Step::Left),
        Button::DPadRight => Some(Step::Right),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Dpad,
    Stick,
}

/// The one direction being held, from whichever pad and control pushed it
/// last, and when it is next due to repeat.
#[derive(Default)]
struct Held {
    by: Option<(PadId, Source, Step)>,
    next: Option<Instant>,
    /// Each pad's stick: which way it is pushed now, to tell a push from the
    /// stream of small changes that follows it.
    sticks: Vec<(PadId, Stick)>,
}

/// One pad's left stick.
#[derive(Default)]
struct Stick {
    pushed: Option<Step>,
    /// Whether each axis, X then Y, has been seen resting. Until then it is
    /// not trusted: measured with a Turtle Beach Recon, a pad nobody has
    /// touched yet reports both sticks hard into a corner, which read as a
    /// push would send the ring sliding left and down on its own.
    rested: [bool; 2],
}

impl Held {
    fn push(&mut self, pad: PadId, source: Source, step: Step, now: Instant) -> Input {
        self.by = Some((pad, source, step));
        self.next = Some(now + REPEAT_DELAY);
        Input::Step(step)
    }

    /// Only the hold that is repeating can end it: letting go of a
    /// direction already overtaken by another changes nothing.
    fn release(&mut self, pad: PadId, source: Source, step: Step) {
        if self.by == Some((pad, source, step)) {
            self.by = None;
            self.next = None;
        }
    }

    /// The held direction, if it is due to repeat by `now`.
    fn repeat(&mut self, now: Instant) -> Option<(PadId, Step)> {
        let (pad, _, step) = self.by?;
        if self.next.is_some_and(|next| now < next) {
            return None;
        }
        self.next = Some(now + REPEAT_EVERY);
        Some((pad, step))
    }

    fn forget(&mut self, pad: PadId) {
        if self.by.is_some_and(|(by, _, _)| by == pad) {
            self.by = None;
            self.next = None;
        }
        self.sticks.retain(|(id, _)| *id != pad);
    }

    /// Follows one pad's stick, and says which way it was just pushed, if
    /// it was. A push ending ends the hold here; a new one is the caller's
    /// to start, once the pad has the floor.
    fn stick(&mut self, pad: PadId, axis: Axis, value: f32) -> Option<Step> {
        let rest = match axis {
            Axis::LeftStickX => 0,
            Axis::LeftStickY => 1,
            _ => return None,
        };
        let index = match self.sticks.iter().position(|(id, _)| *id == pad) {
            Some(index) => index,
            None => {
                self.sticks.push((pad, Stick::default()));
                self.sticks.len() - 1
            }
        };
        let stick = &mut self.sticks[index].1;
        if !stick.rested[rest] {
            stick.rested[rest] = value.abs() < STICK_OFF;
            return None;
        }
        let was = stick.pushed;
        let now_pushed = stick_step(was, axis, value);
        if now_pushed == was {
            return None;
        }
        stick.pushed = now_pushed;
        if let Some(was) = was {
            self.release(pad, Source::Stick, was);
        }
        now_pushed
    }
}

/// Which pad pressed last, and when.
#[derive(Default)]
struct Floor {
    by: Option<(PadId, Instant)>,
}

impl Floor {
    /// Whether `pad` may move the ring now, taking the floor if so: it
    /// already has it, or nobody has pressed anything for [`FLOOR`].
    fn take(&mut self, pad: PadId, now: Instant) -> bool {
        let free = match self.by {
            Some((by, at)) => by == pad || now.duration_since(at) >= FLOOR,
            None => true,
        };
        if free {
            self.by = Some((pad, now));
        }
        free
    }
}

/// Where the stick points after `axis` moved to `value`, given where it
/// pointed before. Only one axis moves per event, so a push along the other
/// one stays as it was.
fn stick_step(was: Option<Step>, axis: Axis, value: f32) -> Option<Step> {
    let (negative, positive) = match axis {
        Axis::LeftStickX => (Step::Left, Step::Right),
        // gilrs reports up as positive.
        Axis::LeftStickY => (Step::Down, Step::Up),
        _ => return was,
    };
    let this_axis = was.filter(|s| *s == negative || *s == positive);
    match this_axis {
        Some(_) if value.abs() >= STICK_OFF => this_axis,
        _ if value >= STICK_ON => Some(positive),
        _ if value <= -STICK_ON => Some(negative),
        // Let go along this axis; a push along the other one is kept.
        Some(_) => None,
        None => was,
    }
}

/// The pads Steam is translating already, from SDL's two variables: an
/// ignore list, or an only-these list. Entries are `0xVVVV/0xPPPP`.
#[derive(Debug, Default)]
struct Ignored {
    skip: HashSet<(u16, u16)>,
    only: Option<HashSet<(u16, u16)>>,
}

impl Ignored {
    fn from_env() -> Self {
        let read = |name| std::env::var(name).ok().map(|value| parse_devices(&value));
        Self {
            skip: read("SDL_GAMECONTROLLER_IGNORE_DEVICES").unwrap_or_default(),
            only: read("SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT").filter(|only| !only.is_empty()),
        }
    }

    fn skips(&self, vendor: Option<u16>, product: Option<u16>) -> bool {
        let (Some(vendor), Some(product)) = (vendor, product) else {
            return false;
        };
        let id = (vendor, product);
        self.skip.contains(&id) || self.only.as_ref().is_some_and(|only| !only.contains(&id))
    }
}

fn parse_devices(value: &str) -> HashSet<(u16, u16)> {
    let hex = |s: &str| {
        let s = s.trim();
        let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
        u16::from_str_radix(s, 16).ok()
    };
    value
        .split(',')
        .filter_map(|entry| {
            let (vendor, product) = entry.split_once('/')?;
            Some((hex(vendor)?, hex(product)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steams_device_list_parses() {
        let devices = parse_devices("0x28DE/0x1205,0x045e/0x028e, 0x2dc8/0x3106,");
        assert_eq!(devices.len(), 3);
        assert!(devices.contains(&(0x28de, 0x1205)));
        assert!(devices.contains(&(0x045e, 0x028e)));
        assert!(parse_devices("").is_empty());
        assert!(parse_devices("junk,0xZZZZ/0x1").is_empty());
    }

    #[test]
    fn only_listed_pads_pass_an_except_list() {
        let ignored = Ignored {
            skip: parse_devices("0x28de/0x1205"),
            only: Some(parse_devices("0x28de/0x11ff")),
        };
        assert!(ignored.skips(Some(0x28de), Some(0x1205)));
        assert!(ignored.skips(Some(0x045e), Some(0x028e)));
        assert!(!ignored.skips(Some(0x28de), Some(0x11ff)));
        // A pad that does not say what it is cannot be on either list.
        assert!(!ignored.skips(None, None));
    }

    #[test]
    fn the_stick_steps_once_per_push_with_room_to_rest() {
        assert_eq!(stick_step(None, Axis::LeftStickX, 0.3), None);
        assert_eq!(stick_step(None, Axis::LeftStickX, 0.7), Some(Step::Right));
        // Drifting back between the two thresholds is still a push.
        assert_eq!(stick_step(Some(Step::Right), Axis::LeftStickX, 0.5), Some(Step::Right));
        assert_eq!(stick_step(Some(Step::Right), Axis::LeftStickX, 0.2), None);
        assert_eq!(stick_step(None, Axis::LeftStickY, 0.9), Some(Step::Up));
        assert_eq!(stick_step(None, Axis::LeftStickY, -0.9), Some(Step::Down));
        // The other axis resting leaves a push along this one alone.
        assert_eq!(stick_step(Some(Step::Up), Axis::LeftStickX, 0.1), Some(Step::Up));
        assert_eq!(stick_step(Some(Step::Up), Axis::RightStickX, 0.9), Some(Step::Up));
    }

    #[test]
    fn a_stick_counts_only_once_it_has_rested() {
        let pad = 0;
        let mut held = Held::default();
        // What an untouched pad reported on connecting.
        assert_eq!(held.stick(pad, Axis::LeftStickX, -1.0), None);
        assert_eq!(held.stick(pad, Axis::LeftStickY, -1.0), None);
        assert_eq!(held.stick(pad, Axis::LeftStickX, 0.0), None);
        assert_eq!(held.stick(pad, Axis::LeftStickX, -0.9), Some(Step::Left));
        // Y has still never rested.
        assert_eq!(held.stick(pad, Axis::LeftStickY, 0.9), None);
    }

    #[test]
    fn a_hold_repeats_only_when_due() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut held = Held::default();
        held.push(0, Source::Dpad, Step::Down, at(0));
        // gilrs coming back empty early, as it does for filtered noise.
        assert_eq!(held.repeat(at(4)), None);
        assert_eq!(held.repeat(at(399)), None);
        assert_eq!(held.repeat(at(400)), Some((0, Step::Down)));
        assert_eq!(held.repeat(at(404)), None);
        assert_eq!(held.repeat(at(530)), Some((0, Step::Down)));
        held.release(0, Source::Dpad, Step::Down);
        assert_eq!(held.repeat(at(700)), None);
    }

    #[test]
    fn a_mirrored_pad_does_not_press_twice() {
        let (real, steam) = (0, 1);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut floor = Floor::default();
        assert!(floor.take(real, at(0)));
        // Steam's virtual pad, a moment behind with the same press.
        assert!(!floor.take(steam, at(2)));
        // Pressing on keeps the floor with the pad that has it.
        assert!(floor.take(real, at(100)));
        assert!(!floor.take(steam, at(200)));
        // Once nothing has been pressed for a while, any pad may.
        assert!(floor.take(steam, at(400)));
        assert!(!floor.take(real, at(410)));
    }
}
