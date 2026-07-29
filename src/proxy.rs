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
}
