use evdev::{AbsoluteAxisType, EventType, InputEvent};

use crate::fsm::ContactId;
use crate::MAX_MT_SLOTS;

const INACTIVE_TRACKING_ID: i32 = -1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoutingMode {
    #[default]
    Passthrough,
    Capture(ContactId),
}

#[derive(Debug)]
pub struct Router {
    current_slot: usize,
    tracking_ids: [i32; MAX_MT_SLOTS],
}

impl Default for Router {
    fn default() -> Self {
        Self {
            current_slot: 0,
            tracking_ids: [INACTIVE_TRACKING_ID; MAX_MT_SLOTS],
        }
    }
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// Route one SYN_REPORT-bounded physical frame.
    ///
    /// `VirtualDevice::emit()` appends one SYN_REPORT, so the physical
    /// SYN_REPORT itself is omitted. The caller must invoke this once
    /// per physical frame to preserve report boundaries.
    pub fn route_frame(
        &mut self,
        events: &[InputEvent],
        mode: RoutingMode,
        output: &mut Vec<InputEvent>,
    ) {
        output.clear();
        output.reserve(events.len());

        for event in events {
            if event.event_type() == EventType::ABSOLUTE {
                let axis = AbsoluteAxisType(event.code());
                if axis == AbsoluteAxisType::ABS_MT_SLOT && event.value() >= 0 {
                    self.current_slot = event.value() as usize;
                }
                if axis == AbsoluteAxisType::ABS_MT_TRACKING_ID {
                    if let Some(tracking_id) = self.tracking_ids.get_mut(self.current_slot) {
                        *tracking_id = event.value();
                    }
                }
                if self.suppress_position(axis, mode) {
                    continue;
                }
            }

            if event.event_type() == EventType::SYNCHRONIZATION && event.code() == 0 {
                continue;
            }
            output.push(*event);
        }
    }

    fn suppress_position(&self, axis: AbsoluteAxisType, mode: RoutingMode) -> bool {
        let RoutingMode::Capture(contact) = mode else {
            return false;
        };

        match axis {
            AbsoluteAxisType::ABS_X | AbsoluteAxisType::ABS_Y => true,
            AbsoluteAxisType::ABS_MT_POSITION_X | AbsoluteAxisType::ABS_MT_POSITION_Y => {
                self.current_slot == contact.slot
                    && self.tracking_ids.get(self.current_slot).copied()
                        == Some(contact.tracking_id)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use evdev::{Key, MiscType};

    use super::*;

    const CAPTURED: ContactId = ContactId {
        slot: 2,
        tracking_id: 20,
    };

    fn abs(axis: AbsoluteAxisType, value: i32) -> InputEvent {
        InputEvent::new(EventType::ABSOLUTE, axis.0, value)
    }

    fn key(key: Key, value: i32) -> InputEvent {
        InputEvent::new(EventType::KEY, key.0, value)
    }

    fn syn() -> InputEvent {
        InputEvent::new(EventType::SYNCHRONIZATION, 0, 0)
    }

    fn with_report(mut events: Vec<InputEvent>) -> Vec<InputEvent> {
        events.push(syn());
        events
    }

    fn restore_report(events: &[InputEvent]) -> Vec<InputEvent> {
        with_report(events.to_vec())
    }

    fn semantics(events: &[InputEvent]) -> Vec<(u16, u16, i32)> {
        events
            .iter()
            .map(|event| (event.event_type().0, event.code(), event.value()))
            .collect()
    }

    #[test]
    fn passthrough_preserves_semantics_order_and_report_boundary() {
        let input = vec![
            key(Key::BTN_LEFT, 1),
            abs(AbsoluteAxisType::ABS_MT_SLOT, 2),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, 20),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 700),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 500),
            abs(AbsoluteAxisType::ABS_MT_PRESSURE, 50),
            InputEvent::new(EventType::MISC, MiscType::MSC_TIMESTAMP.0, 123),
            key(Key::BTN_LEFT, 0),
            syn(),
        ];
        let mut router = Router::new();
        let mut output = Vec::new();

        router.route_frame(&input, RoutingMode::Passthrough, &mut output);

        assert_eq!(semantics(&restore_report(&output)), semantics(&input));
    }

    #[test]
    fn capture_suppresses_only_captured_mt_position_and_primary_mirror() {
        let input = vec![
            abs(AbsoluteAxisType::ABS_X, 777),
            abs(AbsoluteAxisType::ABS_Y, 555),
            abs(AbsoluteAxisType::ABS_MT_SLOT, 2),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, 20),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 700),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 500),
            abs(AbsoluteAxisType::ABS_MT_PRESSURE, 60),
            abs(AbsoluteAxisType::ABS_MT_TOUCH_MAJOR, 30),
            abs(AbsoluteAxisType::ABS_MT_ORIENTATION, 1),
            abs(AbsoluteAxisType::ABS_MT_SLOT, 4),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 300),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 400),
            syn(),
        ];
        let expected = vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, 2),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, 20),
            abs(AbsoluteAxisType::ABS_MT_PRESSURE, 60),
            abs(AbsoluteAxisType::ABS_MT_TOUCH_MAJOR, 30),
            abs(AbsoluteAxisType::ABS_MT_ORIENTATION, 1),
            abs(AbsoluteAxisType::ABS_MT_SLOT, 4),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 300),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 400),
        ];
        let mut router = Router::new();
        let mut output = Vec::new();

        router.route_frame(&input, RoutingMode::Capture(CAPTURED), &mut output);

        assert_eq!(semantics(&output), semantics(&expected));
    }

    #[test]
    fn selected_slot_context_survives_report_boundaries() {
        let mut router = Router::new();
        let mut output = Vec::new();
        router.route_frame(
            &with_report(vec![
                abs(AbsoluteAxisType::ABS_MT_SLOT, 2),
                abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, 20),
            ]),
            RoutingMode::Passthrough,
            &mut output,
        );

        router.route_frame(
            &with_report(vec![
                abs(AbsoluteAxisType::ABS_MT_POSITION_X, 701),
                abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 501),
            ]),
            RoutingMode::Capture(CAPTURED),
            &mut output,
        );

        assert!(output.is_empty());
    }

    #[test]
    fn capture_never_suppresses_buttons_or_tracking_lifecycle() {
        let input = with_report(vec![
            key(Key::BTN_LEFT, 1),
            key(Key::BTN_RIGHT, 1),
            key(Key::BTN_TOUCH, 0),
            key(Key::BTN_TOOL_FINGER, 0),
            abs(AbsoluteAxisType::ABS_MT_SLOT, 2),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
            key(Key::BTN_LEFT, 0),
            key(Key::BTN_RIGHT, 0),
        ]);
        let expected = input[..input.len() - 1].to_vec();
        let mut router = Router::new();
        let mut output = Vec::new();

        router.route_frame(&input, RoutingMode::Capture(CAPTURED), &mut output);

        assert_eq!(semantics(&output), semantics(&expected));
    }

    #[test]
    fn reused_captured_slot_forwards_the_new_contacts_initial_position() {
        let input = with_report(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, CAPTURED.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, 31),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 640),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 480),
        ]);
        let expected = input[..input.len() - 1].to_vec();
        let mut router = Router::new();
        let mut output = Vec::new();
        router.route_frame(
            &with_report(vec![
                abs(AbsoluteAxisType::ABS_MT_SLOT, CAPTURED.slot as i32),
                abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, CAPTURED.tracking_id),
            ]),
            RoutingMode::Passthrough,
            &mut output,
        );

        router.route_frame(&input, RoutingMode::Capture(CAPTURED), &mut output);

        assert_eq!(semantics(&output), semantics(&expected));
    }

    #[test]
    fn cursor_mirror_resumes_after_capture() {
        let mut router = Router::new();
        let mut output = Vec::new();
        let mirror = with_report(vec![
            abs(AbsoluteAxisType::ABS_X, 710),
            abs(AbsoluteAxisType::ABS_Y, 510),
        ]);

        router.route_frame(&mirror, RoutingMode::Capture(CAPTURED), &mut output);
        assert!(output.is_empty());

        router.route_frame(&mirror, RoutingMode::Passthrough, &mut output);
        assert_eq!(semantics(&output), semantics(&mirror[..mirror.len() - 1]));
    }
}
