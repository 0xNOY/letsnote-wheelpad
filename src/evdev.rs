// Physical touchpad input — opens the evdev node, queries EVIOCGABS for
// center coordinates, and turns the raw event stream into TouchFrames at
// each SYN_REPORT. See linux-design.md §5.

use std::path::{Path, PathBuf};

use evdev::{AbsoluteAxisType, Device, EventType, InputEvent, Key};

use crate::detector::TouchSample;
use crate::error::{Error, Result};
use crate::fsm::{ContactId, TouchFrame};

/// Maximum number of MT slots we track. The kernel exposes up to 10 in
/// practice; touchpads typically advertise 5. We sweep all slots when
/// rescanning for "lowest active" semantics (D-012), so the constant
/// only bounds the per-frame map size, not gesture logic.
const MAX_MT_SLOTS: usize = 16;

pub struct InputDevice {
    pub device: Device,
    pub abs_x_min: i32,
    pub abs_x_max: i32,
    pub abs_y_min: i32,
    pub abs_y_max: i32,
    pub center_x: i32,
    pub center_y: i32,
    state: FrameState,
}

#[derive(Clone, Copy, Debug)]
struct FrameState {
    /// Per-slot tracking IDs and last-seen (x, y). Slot index 0..MAX_MT_SLOTS-1.
    slots: [SlotState; MAX_MT_SLOTS],
    /// Current writing slot per ABS_MT_SLOT event.
    current_slot: usize,
    /// BTN_TOUCH summary; mirrors the kernel-reported any-finger-down state.
    contact: bool,
}

#[derive(Clone, Copy, Debug)]
struct SlotState {
    /// `-1` means inactive (kernel convention).
    tracking_id: i32,
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct ContactSnapshot {
    slots: [SlotState; MAX_MT_SLOTS],
    contact: bool,
}

impl ContactSnapshot {
    pub fn primary(&self) -> TouchFrame {
        let contact = self
            .contact
            .then(|| {
                self.slots
                    .iter()
                    .enumerate()
                    .find(|(_, slot)| slot.tracking_id != -1)
            })
            .flatten();
        match contact {
            Some((slot, state)) => TouchFrame {
                contact: true,
                pos: Some(TouchSample {
                    x: state.x,
                    y: state.y,
                }),
                contact_id: Some(ContactId {
                    slot,
                    tracking_id: state.tracking_id,
                }),
            },
            None => TouchFrame {
                contact: false,
                pos: None,
                contact_id: None,
            },
        }
    }

    pub fn for_contact(&self, id: ContactId) -> TouchFrame {
        let active = self
            .slots
            .get(id.slot)
            .filter(|slot| self.contact && slot.tracking_id == id.tracking_id);
        match active {
            Some(state) => TouchFrame {
                contact: true,
                pos: Some(TouchSample {
                    x: state.x,
                    y: state.y,
                }),
                contact_id: Some(id),
            },
            None => TouchFrame {
                contact: false,
                pos: None,
                contact_id: Some(id),
            },
        }
    }
}

/// One SYN_REPORT-bounded batch of physical events plus the
/// high-level frame assembled from them.
pub struct PhysicalFrame {
    pub contacts: ContactSnapshot,
    /// All events from the underlying fetch, in original order.
    /// Forwarded verbatim to the virtual touchpad (minus the trailing
    /// SYN_REPORT, which `emit()` re-inserts).
    pub events: Vec<InputEvent>,
}

impl Drop for InputDevice {
    /// Panic-safe ungrab. The kernel releases EVIOCGRAB on FD close
    /// regardless, but explicitly calling ungrab here means the
    /// release happens deterministically during unwind, before the
    /// FD's file struct is reaped — closing the race where a daemon
    /// restart tries to re-grab before the kernel has finalized the
    /// old FD.
    fn drop(&mut self) {
        let _ = self.device.ungrab();
    }
}

impl InputDevice {
    /// Open the device at `path` and validate required capabilities.
    pub fn open(path: &Path) -> Result<Self> {
        let device = Device::open(path).map_err(|source| Error::EvdevOpen {
            path: path.to_path_buf(),
            source,
        })?;

        // Required capabilities.
        let abs = device.supported_absolute_axes();
        let keys = device.supported_keys();
        let has_x = abs.is_some_and(|a| a.contains(AbsoluteAxisType::ABS_MT_POSITION_X));
        let has_y = abs.is_some_and(|a| a.contains(AbsoluteAxisType::ABS_MT_POSITION_Y));
        let has_touch = keys.is_some_and(|k| k.contains(Key::BTN_TOUCH));
        if !has_x {
            return Err(Error::EvdevMissingCap {
                path: path.to_path_buf(),
                capability: "ABS_MT_POSITION_X",
            });
        }
        if !has_y {
            return Err(Error::EvdevMissingCap {
                path: path.to_path_buf(),
                capability: "ABS_MT_POSITION_Y",
            });
        }
        if !has_touch {
            return Err(Error::EvdevMissingCap {
                path: path.to_path_buf(),
                capability: "BTN_TOUCH",
            });
        }

        let abs_state = device
            .get_abs_state()
            .map_err(|source| Error::EvdevRead { source })?;
        let xi = abs_state[AbsoluteAxisType::ABS_MT_POSITION_X.0 as usize];
        let yi = abs_state[AbsoluteAxisType::ABS_MT_POSITION_Y.0 as usize];
        let abs_x_min = xi.minimum;
        let abs_x_max = xi.maximum;
        let abs_y_min = yi.minimum;
        let abs_y_max = yi.maximum;
        let center_x = (abs_x_min + abs_x_max) / 2;
        let center_y = (abs_y_min + abs_y_max) / 2;

        let slots = [SlotState {
            tracking_id: -1,
            x: 0,
            y: 0,
        }; MAX_MT_SLOTS];

        let state = FrameState {
            slots,
            current_slot: 0,
            contact: false,
        };

        Ok(Self {
            device,
            abs_x_min,
            abs_x_max,
            abs_y_min,
            abs_y_max,
            center_x,
            center_y,
            state,
        })
    }

    /// Find a touchpad whose name matches `regex`. Returns the first match
    /// found via `/dev/input/event*` enumeration.
    pub fn find_by_name(regex_str: &str) -> Result<PathBuf> {
        let re = regex::Regex::new(regex_str).map_err(|source| Error::RegexInvalid {
            pattern: regex_str.to_string(),
            source,
        })?;
        for (path, device) in evdev::enumerate() {
            if let Some(name) = device.name() {
                if re.is_match(name) {
                    return Ok(path);
                }
            }
        }
        Err(Error::DeviceNotFound {
            regex: regex_str.to_string(),
        })
    }

    /// Block until events are available, then return ALL complete
    /// SYN_REPORT-bounded frames from this fetch. If the daemon was
    /// briefly descheduled and the kernel has buffered multiple
    /// frames, every one is returned so the caller can step the FSM
    /// and forward to the virtual touchpad per-frame — gluing N
    /// kernel batches into one virtual batch loses per-frame timing
    /// and corrupts libinput's state.
    ///
    /// evdev 0.12.2's synchronized iterator only yields complete
    /// SYN_REPORT-terminated blocks. An incomplete kernel read remains
    /// in the library's internal buffer for the next `fetch_events()`.
    pub fn poll_frames(&mut self) -> Result<Vec<PhysicalFrame>> {
        let events: Vec<InputEvent> = self
            .device
            .fetch_events()
            .map_err(|source| Error::EvdevRead { source })?
            .collect();
        let mut frames: Vec<PhysicalFrame> = Vec::new();
        let mut batch_events: Vec<InputEvent> = Vec::new();
        for ev in events {
            batch_events.push(ev);
            match ev.event_type() {
                EventType::ABSOLUTE => self.state.apply_abs(ev.code(), ev.value()),
                EventType::KEY if ev.code() == Key::BTN_TOUCH.code() => {
                    self.state.contact = ev.value() != 0;
                }
                EventType::SYNCHRONIZATION if ev.code() == 0 => {
                    frames.push(PhysicalFrame {
                        contacts: self.state.snapshot(),
                        events: std::mem::take(&mut batch_events),
                    });
                }
                _ => {}
            }
        }
        Ok(frames)
    }
}

impl FrameState {
    fn apply_abs(&mut self, code: u16, value: i32) {
        let axis = AbsoluteAxisType(code);
        match axis {
            AbsoluteAxisType::ABS_MT_SLOT if (value as usize) < MAX_MT_SLOTS => {
                self.current_slot = value as usize;
            }
            AbsoluteAxisType::ABS_MT_TRACKING_ID => {
                let slot = self.current_slot;
                if slot < MAX_MT_SLOTS {
                    self.slots[slot].tracking_id = value;
                }
            }
            AbsoluteAxisType::ABS_MT_POSITION_X => {
                let slot = self.current_slot;
                if slot < MAX_MT_SLOTS {
                    self.slots[slot].x = value;
                }
            }
            AbsoluteAxisType::ABS_MT_POSITION_Y => {
                let slot = self.current_slot;
                if slot < MAX_MT_SLOTS {
                    self.slots[slot].y = value;
                }
            }
            // Some pads also expose ABS_X / ABS_Y for the primary touch.
            // We deliberately ignore those — MT axes are authoritative.
            _ => {}
        }
    }

    fn snapshot(&self) -> ContactSnapshot {
        ContactSnapshot {
            slots: self.slots,
            contact: self.contact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> FrameState {
        FrameState {
            slots: [SlotState {
                tracking_id: -1,
                x: 0,
                y: 0,
            }; MAX_MT_SLOTS],
            current_slot: 0,
            contact: true,
        }
    }

    fn set_contact(state: &mut FrameState, slot: usize, tracking_id: i32, x: i32, y: i32) {
        state.apply_abs(AbsoluteAxisType::ABS_MT_SLOT.0, slot as i32);
        state.apply_abs(AbsoluteAxisType::ABS_MT_TRACKING_ID.0, tracking_id);
        state.apply_abs(AbsoluteAxisType::ABS_MT_POSITION_X.0, x);
        state.apply_abs(AbsoluteAxisType::ABS_MT_POSITION_Y.0, y);
    }

    #[test]
    fn tracked_contact_does_not_switch_to_a_lower_slot() {
        let mut state = state();
        set_contact(&mut state, 3, 30, 800, 500);
        let tracked = state.snapshot().primary().contact_id.unwrap();

        set_contact(&mut state, 1, 10, 200, 300);
        let frame = state.snapshot().for_contact(tracked);

        assert_eq!(frame.contact_id, Some(tracked));
        assert_eq!(frame.pos, Some(TouchSample { x: 800, y: 500 }));
    }

    #[test]
    fn only_the_tracked_tracking_id_ending_lifts_the_session_contact() {
        let mut state = state();
        set_contact(&mut state, 3, 30, 800, 500);
        set_contact(&mut state, 1, 10, 200, 300);
        let tracked = ContactId {
            slot: 3,
            tracking_id: 30,
        };

        state.apply_abs(AbsoluteAxisType::ABS_MT_SLOT.0, 1);
        state.apply_abs(AbsoluteAxisType::ABS_MT_TRACKING_ID.0, -1);
        assert!(state.snapshot().for_contact(tracked).contact);

        state.apply_abs(AbsoluteAxisType::ABS_MT_SLOT.0, 3);
        state.apply_abs(AbsoluteAxisType::ABS_MT_TRACKING_ID.0, -1);
        assert!(!state.snapshot().for_contact(tracked).contact);
    }

    #[test]
    fn a_reused_slot_does_not_inherit_the_old_contact() {
        let mut state = state();
        set_contact(&mut state, 3, 30, 800, 500);
        let tracked = ContactId {
            slot: 3,
            tracking_id: 30,
        };
        set_contact(&mut state, 3, -1, 800, 500);
        set_contact(&mut state, 3, 31, 700, 400);

        assert!(!state.snapshot().for_contact(tracked).contact);
    }
}
