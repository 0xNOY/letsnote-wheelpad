// 6-state FSM mirroring FUN_1400046a0 — see analysis/RE-findings.md §4
// and analysis/linux-design.md §5.

use crate::config::Scroll;
use crate::detector::{
    engagement_swept_angle, radial_gate_ok, within_horizontal_arc, CircularDetector,
    CoordinateTransform, TouchSample, TRIGGER_ANGLE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactId {
    pub slot: usize,
    pub tracking_id: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FsmState {
    Idle,
    Contact {
        origin: TouchSample,
        contact: ContactId,
    },
    Moving {
        engage_start: TouchSample,
        contact: ContactId,
    },
    Scrolling {
        contact: ContactId,
    },
    // The FSM has an explicit Debounce state to preserve the structure
    // of the original Windows WheelPad FSM (FUN_1400046a0 case 5).
    //
    // The Windows timer expression decompiled to `abs(int) < 1`, which
    // is literally "true only on the same millisecond as the lift" —
    // almost certainly a Ghidra-reconstruction artifact for a bound
    // check that was originally `< CONST_TIMEOUT_MS`.
    //
    // Since the literal expression makes the timer-still-active branch
    // effectively unreachable on Windows, real Windows users do not
    // experience debounce-based quick relift. We mirror that: enter
    // Debounce on lift and exit to Idle on the very next frame, with no
    // timer check. We deliberately do NOT expose a TOML knob for this —
    // see DECISIONS.md D-011-followup.
    Debounce,
}

#[derive(Clone, Copy, Debug)]
pub struct TouchFrame {
    pub contact: bool,
    pub pos: Option<TouchSample>,
    pub contact_id: Option<ContactId>,
}

/// Side effects the FSM asks the runtime to perform. The runtime also
/// derives event forwarding (passthrough) directly from `Fsm::state()`,
/// so the FSM does NOT emit "start/stop grabbing" actions any more —
/// the physical pad is grabbed permanently at startup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    None,
    EmitWheelV(i32),
    EmitWheelH(i32),
}

pub struct Fsm {
    state: FsmState,
    center_x: i32,
    center_y: i32,
    transform: CoordinateTransform,
}

impl Fsm {
    pub fn new(center_x: i32, center_y: i32) -> Self {
        Self::with_transform(center_x, center_y, CoordinateTransform::default())
    }

    pub fn with_transform(center_x: i32, center_y: i32, transform: CoordinateTransform) -> Self {
        Self {
            state: FsmState::Idle,
            center_x,
            center_y,
            transform,
        }
    }

    pub fn state(&self) -> FsmState {
        self.state
    }

    pub fn contact_id(&self) -> Option<ContactId> {
        match self.state {
            FsmState::Contact { contact, .. }
            | FsmState::Moving { contact, .. }
            | FsmState::Scrolling { contact } => Some(contact),
            FsmState::Idle | FsmState::Debounce => None,
        }
    }

    /// Advance the FSM by one touch frame. Mutates the supplied detector
    /// (which holds the chord-angle accumulator and 20-unit-dead-band
    /// shift register) and the scroll config. Returns the (at most one)
    /// wheel-emission action the runtime needs to perform.
    ///
    /// The runtime derives event-forwarding decisions (suppress motion
    /// to the virtual touchpad during scroll) directly from
    /// [`Fsm::state`] after this call returns; no grab/release actions
    /// flow through this return value any more.
    pub fn step(
        &mut self,
        frame: TouchFrame,
        detector: &mut CircularDetector,
        scroll: &Scroll,
    ) -> Action {
        // Master gate. If scrolling is disabled at the config level we
        // still consume frames so the daemon stays alive (D-007), but
        // never advance past Idle and never emit ticks.
        if !scroll.enable {
            self.state = FsmState::Idle;
            return Action::None;
        }

        match (self.state, frame.contact, frame.pos) {
            // ---------- Idle (state 1) ----------
            (FsmState::Idle, false, _) | (FsmState::Idle, true, None) => Action::None,
            (FsmState::Idle, true, Some(s)) => {
                let Some(contact) = frame.contact_id else {
                    return Action::None;
                };
                // Fresh touch-down: radial-gate classifier (FUN_140005a00).
                if radial_gate_ok(
                    self.center_x,
                    self.center_y,
                    s,
                    scroll.detect_area_width,
                    scroll.detect_area_radius,
                    self.transform,
                ) {
                    // Outside dead zone → MOVING. Capture engage_start
                    // here, matching DAT_14003cc18 being set at
                    // FUN_1400046a0 line 203 only on the state 1 → state 3
                    // transition. The detector's accumulator and history
                    // are NOT reset here; that happens on the
                    // Moving → Scrolling transition (mirroring
                    // FUN_1400046a0 line 151 which zeros DAT_14003cb00).
                    self.state = FsmState::Moving {
                        engage_start: s,
                        contact,
                    };
                } else {
                    // Inside dead zone → CONTACT (trap).
                    self.state = FsmState::Contact { origin: s, contact };
                }
                Action::None
            }

            // ---------- Contact (state 2) — dead-zone trap (D-020) ----------
            (FsmState::Contact { .. }, false, _) => {
                // Finger lifted. Per FUN_1400046a0 case 2 lines 118-126,
                // Contact only exits to Idle on lift; cross-gate movement
                // while in Contact does NOT transition to Moving. See
                // DECISIONS.md D-020.
                self.state = FsmState::Idle;
                Action::None
            }
            (FsmState::Contact { .. }, true, _) => {
                // Stay trapped regardless of where the finger is now.
                Action::None
            }

            // ---------- Moving (state 3) — engagement candidate ----------
            (FsmState::Moving { .. }, false, _) | (FsmState::Moving { .. }, true, None) => {
                // Lift before engagement → Idle.
                self.state = FsmState::Idle;
                Action::None
            }
            (
                FsmState::Moving {
                    engage_start,
                    contact,
                },
                true,
                Some(s),
            ) => {
                if !radial_gate_ok(
                    self.center_x,
                    self.center_y,
                    s,
                    scroll.detect_area_width,
                    scroll.detect_area_radius,
                    self.transform,
                ) {
                    // Slipped back into the dead zone — fall back to
                    // Contact (FUN_1400046a0 case 3, lines 127-137).
                    self.state = FsmState::Contact { origin: s, contact };
                    Action::None
                } else {
                    let swept = engagement_swept_angle(
                        self.center_x,
                        self.center_y,
                        engage_start,
                        s,
                        self.transform,
                    );
                    if swept.abs() > TRIGGER_ANGLE {
                        // Engagement! Reset detector and enter Scrolling.
                        // The physical pad is already grabbed (forever);
                        // forwarding suppression is keyed off state.
                        detector.on_gesture_start();
                        self.state = FsmState::Scrolling { contact };
                        // Feed the engaging sample so the first tick can
                        // emit on this very frame if the gesture is fast
                        // enough.
                        detector.push_if_moved(s);
                        let ticks = detector.step(scroll.sensitivity);
                        if ticks != 0 {
                            emit(
                                ticks,
                                scroll,
                                self.center_x,
                                self.center_y,
                                s,
                                self.transform,
                            )
                        } else {
                            Action::None
                        }
                    } else {
                        // Stay in Moving until lift, slip-back, or
                        // swept-angle threshold.
                        Action::None
                    }
                }
            }

            // ---------- Scrolling (state 4) ----------
            (FsmState::Scrolling { .. }, false, _) | (FsmState::Scrolling { .. }, true, None) => {
                // Lift → Debounce. State change alone is enough for the
                // passthrough runtime to resume forwarding the lift
                // events to the virtual touchpad.
                self.state = FsmState::Debounce;
                Action::None
            }
            (FsmState::Scrolling { .. }, true, Some(s)) => {
                detector.push_if_moved(s);
                let ticks = detector.step(scroll.sensitivity);
                if ticks != 0 {
                    emit(
                        ticks,
                        scroll,
                        self.center_x,
                        self.center_y,
                        s,
                        self.transform,
                    )
                } else {
                    Action::None
                }
            }

            // ---------- Debounce (state 5) — structural marker only ----------
            (FsmState::Debounce, _, _) => {
                // Always transition to Idle on the next frame, regardless
                // of whether the finger is now down or up. See
                // DECISIONS.md D-011-followup and the comment on
                // FsmState::Debounce above.
                self.state = FsmState::Idle;
                Action::None
            }
        }
    }

    /// Explicitly reset state to Idle and clear the detector's
    /// accumulator and history. The runtime does not call this on a
    /// timer: a Scrolling session ends only when its tracked contact
    /// lifts.
    pub fn force_idle(&mut self, detector: &mut CircularDetector) {
        self.state = FsmState::Idle;
        detector.on_gesture_start();
    }
}

/// Apply reverse flags and the arc gate, then return the appropriate
/// EmitWheelV / EmitWheelH action. The horizontal arc is only consulted
/// when `horizontal_enable = true` — vertical scroll is never angle-gated
/// (linux-design.md §5 "Vertical scroll is NOT angle-gated").
fn emit(
    ticks: i32,
    scroll: &Scroll,
    center_x: i32,
    center_y: i32,
    current: TouchSample,
    transform: CoordinateTransform,
) -> Action {
    if scroll.horizontal_enable
        && within_horizontal_arc(
            center_x,
            center_y,
            current,
            scroll.horizontal_start,
            scroll.horizontal_end,
            transform,
        )
    {
        let signed = if scroll.reverse_horizontal {
            -ticks
        } else {
            ticks
        };
        Action::EmitWheelH(signed)
    } else {
        let signed = if scroll.reverse_vertical {
            -ticks
        } else {
            ticks
        };
        Action::EmitWheelV(signed)
    }
}
