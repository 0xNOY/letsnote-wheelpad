use std::io;

use evdev::InputEvent;

use crate::config::Scroll;
use crate::detector::{CircularDetector, CoordinateTransform};
use crate::evdev::{ContactSnapshot, PhysicalFrame};
use crate::fsm::{Action, ContactId, Fsm, FsmState};
use crate::router::{Router, RoutingMode};

pub struct FrameProcessor {
    fsm: Fsm,
    detector: CircularDetector,
    router: Router,
}

impl FrameProcessor {
    pub fn new(
        center_x: i32,
        center_y: i32,
        transform: CoordinateTransform,
        minimum_rotation_radius: f64,
        initial: &ContactSnapshot,
    ) -> Self {
        Self {
            fsm: Fsm::with_transform(center_x, center_y, transform),
            detector: CircularDetector::with_geometry(transform, minimum_rotation_radius),
            router: Router::from_snapshot(initial),
        }
    }

    pub fn process_frame(
        &mut self,
        frame: &PhysicalFrame,
        scroll: &Scroll,
        routed_events: &mut Vec<InputEvent>,
    ) -> io::Result<Action> {
        let previous = self.fsm.state();
        let tracked_before = self.fsm.contact_id();
        let touch = match tracked_before {
            Some(contact) => frame.contacts.for_contact(contact),
            None => frame.contacts.primary(),
        };
        let action = self.fsm.step(touch, &mut self.detector, scroll);
        let current = self.fsm.state();
        let capture = if matches!(previous, FsmState::Scrolling { .. }) {
            tracked_before
        } else if matches!(current, FsmState::Scrolling { .. }) {
            self.fsm.contact_id()
        } else {
            None
        };
        self.router.route_frame(
            &frame.events,
            capture.map_or(RoutingMode::Passthrough, RoutingMode::Capture),
            routed_events,
        )?;
        if frame.reconciled {
            self.router.reconcile(&frame.contacts);
        }
        Ok(action)
    }

    pub fn state(&self) -> FsmState {
        self.fsm.state()
    }

    pub fn tracked_contact(&self) -> Option<ContactId> {
        self.fsm.contact_id()
    }

    pub fn is_scrolling(&self) -> bool {
        matches!(self.fsm.state(), FsmState::Scrolling { .. })
    }

    #[cfg(test)]
    fn router_selected_slot(&self) -> Option<usize> {
        self.router.selected_slot()
    }
}

#[cfg(test)]
mod tests {
    use evdev::{AbsoluteAxisType, EventType, InputEvent};

    use super::*;
    use crate::evdev::{ContactSnapshot, PhysicalFrame};
    use crate::uinput::mt_slot_event;

    struct VirtualTypeB {
        current_slot: usize,
        positions: Vec<(i32, i32)>,
    }

    impl VirtualTypeB {
        fn new(slot_count: usize) -> Self {
            Self {
                current_slot: 0,
                positions: vec![(0, 0); slot_count],
            }
        }

        fn apply(&mut self, events: &[InputEvent]) {
            for event in events {
                if event.event_type() != EventType::ABSOLUTE {
                    continue;
                }
                match AbsoluteAxisType(event.code()) {
                    AbsoluteAxisType::ABS_MT_SLOT => {
                        self.current_slot = usize::try_from(event.value()).unwrap();
                    }
                    AbsoluteAxisType::ABS_MT_POSITION_X => {
                        self.positions[self.current_slot].0 = event.value();
                    }
                    AbsoluteAxisType::ABS_MT_POSITION_Y => {
                        self.positions[self.current_slot].1 = event.value();
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn startup_aligns_router_and_virtual_selected_slot_before_position_only_frame() {
        let confirmed =
            ContactSnapshot::from_slot_values(&[(-1, 0, 0); 4], Some(2), false).unwrap();
        let mut virtual_device = VirtualTypeB::new(confirmed.slot_count());
        virtual_device.apply(&[mt_slot_event(confirmed.selected_slot().unwrap()).unwrap()]);
        let mut processor = FrameProcessor::new(
            500,
            500,
            CoordinateTransform::default(),
            Scroll::default().minimum_rotation_radius,
            &confirmed,
        );
        let physical = PhysicalFrame::from_parts(
            confirmed,
            vec![
                InputEvent::new(
                    EventType::ABSOLUTE,
                    AbsoluteAxisType::ABS_MT_POSITION_X.0,
                    700,
                ),
                InputEvent::new(
                    EventType::ABSOLUTE,
                    AbsoluteAxisType::ABS_MT_POSITION_Y.0,
                    400,
                ),
                InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
            ],
            false,
        );
        let mut routed = Vec::new();

        processor
            .process_frame(&physical, &Scroll::default(), &mut routed)
            .unwrap();
        virtual_device.apply(&routed);

        assert_eq!(processor.router_selected_slot(), Some(2));
        assert_eq!(virtual_device.current_slot, 2);
        assert_eq!(virtual_device.positions[2], (700, 400));
        assert_eq!(virtual_device.positions[0], (0, 0));
    }
}
