use std::f64::consts::PI;

use evdev::{AbsoluteAxisType, EventType, InputEvent, Key};
use letsnote_wheelpad::config::Scroll;
use letsnote_wheelpad::detector::CoordinateTransform;
use letsnote_wheelpad::evdev::{ContactSnapshot, PhysicalFrame};
use letsnote_wheelpad::fsm::{Action, ContactId, FsmState};
use letsnote_wheelpad::proxy::FrameProcessor;

const CONTACT_A: ContactId = ContactId {
    slot: 2,
    tracking_id: 20,
};
const SLOT_COUNT: usize = 5;

struct Harness {
    processor: FrameProcessor,
    routed: Vec<InputEvent>,
    all_routed: Vec<InputEvent>,
}

impl Harness {
    fn new(initial: ContactSnapshot) -> Self {
        Self {
            processor: FrameProcessor::new(
                500,
                500,
                CoordinateTransform::default(),
                Scroll::default().minimum_rotation_radius,
                &initial,
            ),
            routed: Vec::new(),
            all_routed: Vec::new(),
        }
    }

    fn process(
        &mut self,
        contacts: ContactSnapshot,
        events: Vec<InputEvent>,
        reconciled: bool,
    ) -> Action {
        let frame = PhysicalFrame::from_parts(contacts, events, reconciled);
        let action = self
            .processor
            .process_frame(&frame, &Scroll::default(), &mut self.routed)
            .unwrap();
        self.all_routed.extend(self.routed.iter().copied());
        action
    }
}

fn empty_snapshot() -> ContactSnapshot {
    ContactSnapshot::from_slot_values(&[(-1, 0, 0); SLOT_COUNT], Some(0), false).unwrap()
}

fn snapshot_a(angle: f64) -> ContactSnapshot {
    let mut slots = [(-1, 0, 0); SLOT_COUNT];
    slots[CONTACT_A.slot] = (
        CONTACT_A.tracking_id,
        500 + (300.0 * angle.cos()).round() as i32,
        500 + (300.0 * angle.sin()).round() as i32,
    );
    ContactSnapshot::from_slot_values(&slots, Some(CONTACT_A.slot), true).unwrap()
}

fn abs(axis: AbsoluteAxisType, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE, axis.0, value)
}

fn key(key: Key, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY, key.0, value)
}

fn frame(mut events: Vec<InputEvent>) -> Vec<InputEvent> {
    events.push(InputEvent::new(EventType::SYNCHRONIZATION, 0, 0));
    events
}

fn start_and_engage(harness: &mut Harness) {
    harness.process(
        snapshot_a(0.0),
        frame(vec![
            key(Key::BTN_TOUCH, 1),
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, CONTACT_A.tracking_id),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 800),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 500),
        ]),
        false,
    );
    harness.process(
        snapshot_a(PI / 8.0),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 777),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 615),
        ]),
        false,
    );
    assert!(harness.processor.is_scrolling());
}

#[test]
fn production_processor_suppresses_engagement_position() {
    let mut harness = Harness::new(empty_snapshot());
    start_and_engage(&mut harness);

    assert!(!harness.routed.iter().any(|event| {
        event.event_type() == EventType::ABSOLUTE
            && matches!(
                AbsoluteAxisType(event.code()),
                AbsoluteAxisType::ABS_MT_POSITION_X | AbsoluteAxisType::ABS_MT_POSITION_Y
            )
    }));
}

#[test]
fn button_pair_and_captured_lift_lifecycle_are_forwarded() {
    let mut harness = Harness::new(empty_snapshot());
    harness.process(
        snapshot_a(0.0),
        frame(vec![
            key(Key::BTN_LEFT, 1),
            key(Key::BTN_TOUCH, 1),
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, CONTACT_A.tracking_id),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 800),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 500),
        ]),
        false,
    );
    harness.process(
        snapshot_a(PI / 8.0),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 777),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 615),
        ]),
        false,
    );
    harness.process(
        snapshot_a(PI / 8.0),
        frame(vec![key(Key::BTN_LEFT, 0)]),
        false,
    );
    assert!(harness.processor.is_scrolling());

    harness.process(
        empty_snapshot(),
        frame(vec![
            key(Key::BTN_TOUCH, 0),
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
        ]),
        false,
    );

    assert!(matches!(harness.processor.state(), FsmState::Debounce));
    let values = harness
        .all_routed
        .iter()
        .filter(|event| event.event_type() == EventType::KEY && event.code() == Key::BTN_LEFT.0)
        .map(InputEvent::value)
        .collect::<Vec<_>>();
    assert_eq!(values, vec![1, 0]);
    assert!(harness.all_routed.iter().any(|event| {
        event.code() == AbsoluteAxisType::ABS_MT_TRACKING_ID.0 && event.value() == -1
    }));
}

#[test]
fn second_contact_add_move_and_lift_remain_routed() {
    let contact_b = ContactId {
        slot: 0,
        tracking_id: 10,
    };
    let mut harness = Harness::new(empty_snapshot());
    start_and_engage(&mut harness);
    let mut slots = [(-1, 0, 0); SLOT_COUNT];
    slots[CONTACT_A.slot] = (CONTACT_A.tracking_id, 777, 615);
    slots[contact_b.slot] = (contact_b.tracking_id, 200, 300);
    harness.process(
        ContactSnapshot::from_slot_values(&slots, Some(contact_b.slot), true).unwrap(),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, contact_b.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, contact_b.tracking_id),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 200),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 300),
        ]),
        false,
    );
    slots[contact_b.slot] = (-1, 220, 320);
    harness.process(
        ContactSnapshot::from_slot_values(&slots, Some(contact_b.slot), true).unwrap(),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 220),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 320),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
        ]),
        false,
    );

    assert_eq!(harness.processor.tracked_contact(), Some(CONTACT_A));
    assert!(harness.processor.is_scrolling());
    assert!(harness.all_routed.iter().any(|event| {
        event.code() == AbsoluteAxisType::ABS_MT_POSITION_X.0 && event.value() == 220
    }));
}

#[test]
fn lost_sync_forwards_reconstructed_non_captured_positions_while_scrolling() {
    let contact_b = ContactId {
        slot: 0,
        tracking_id: 10,
    };
    let mut harness = Harness::new(empty_snapshot());
    start_and_engage(&mut harness);
    let mut slots = [(-1, 0, 0); SLOT_COUNT];
    slots[CONTACT_A.slot] = (CONTACT_A.tracking_id, 777, 615);
    slots[contact_b.slot] = (contact_b.tracking_id, 200, 300);
    harness.process(
        ContactSnapshot::from_slot_values(&slots, Some(CONTACT_A.slot), true).unwrap(),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, contact_b.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, contact_b.tracking_id),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 200),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 300),
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
        ]),
        false,
    );

    slots[CONTACT_A.slot] = (CONTACT_A.tracking_id, 760, 620);
    slots[contact_b.slot] = (contact_b.tracking_id, 240, 340);
    harness.process(
        ContactSnapshot::from_slot_values(&slots, Some(CONTACT_A.slot), true).unwrap(),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 760),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 620),
            abs(AbsoluteAxisType::ABS_MT_SLOT, contact_b.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 240),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 340),
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
        ]),
        true,
    );

    assert!(harness.processor.is_scrolling());
    assert!(harness.routed.iter().any(|event| {
        event.code() == AbsoluteAxisType::ABS_MT_POSITION_X.0 && event.value() == 240
    }));
    assert!(harness.routed.iter().any(|event| {
        event.code() == AbsoluteAxisType::ABS_MT_POSITION_Y.0 && event.value() == 340
    }));
    assert!(!harness.routed.iter().any(|event| {
        matches!(event.value(), 760 | 620)
            && matches!(
                AbsoluteAxisType(event.code()),
                AbsoluteAxisType::ABS_MT_POSITION_X | AbsoluteAxisType::ABS_MT_POSITION_Y
            )
    }));
}

#[test]
fn same_slot_reuse_is_not_captured() {
    let mut harness = Harness::new(empty_snapshot());
    start_and_engage(&mut harness);
    let mut slots = [(-1, 0, 0); SLOT_COUNT];
    slots[CONTACT_A.slot] = (31, 640, 480);
    harness.process(
        ContactSnapshot::from_slot_values(&slots, Some(CONTACT_A.slot), true).unwrap(),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, 31),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 640),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 480),
        ]),
        false,
    );

    assert!(!harness.processor.is_scrolling());
    assert!(harness.routed.iter().any(|event| {
        event.code() == AbsoluteAxisType::ABS_MT_POSITION_X.0 && event.value() == 640
    }));
}

#[test]
fn stationary_captured_contact_survives_liveness_reconciliation() {
    let mut harness = Harness::new(empty_snapshot());
    start_and_engage(&mut harness);
    harness.process(snapshot_a(PI / 8.0), frame(Vec::new()), true);

    assert!(harness.processor.is_scrolling());
    assert_eq!(harness.processor.tracked_contact(), Some(CONTACT_A));
}

#[test]
fn missing_captured_contact_ends_session_during_liveness_reconciliation() {
    let mut harness = Harness::new(empty_snapshot());
    start_and_engage(&mut harness);
    harness.process(
        empty_snapshot(),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
            key(Key::BTN_TOUCH, 0),
        ]),
        true,
    );

    assert!(!harness.processor.is_scrolling());
    assert!(harness.routed.iter().any(|event| {
        event.code() == AbsoluteAxisType::ABS_MT_TRACKING_ID.0 && event.value() == -1
    }));
}

#[test]
fn startup_with_active_contact_uses_synchronized_identity() {
    let initial = snapshot_a(0.0);
    let mut harness = Harness::new(initial);
    harness.process(
        snapshot_a(0.0),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 800),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 500),
        ]),
        false,
    );
    harness.process(
        snapshot_a(PI / 8.0),
        frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 777),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 615),
        ]),
        false,
    );

    assert!(harness.processor.is_scrolling());
    assert_eq!(harness.processor.tracked_contact(), Some(CONTACT_A));
}
