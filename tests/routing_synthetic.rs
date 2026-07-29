use std::f64::consts::PI;

use evdev::{AbsoluteAxisType, EventType, InputEvent, Key};
use letsnote_wheelpad::config::Scroll;
use letsnote_wheelpad::detector::{CircularDetector, TouchSample};
use letsnote_wheelpad::fsm::{Action, ContactId, Fsm, FsmState, TouchFrame};
use letsnote_wheelpad::router::{Router, RoutingMode};

const CONTACT_A: ContactId = ContactId {
    slot: 2,
    tracking_id: 20,
};

fn touch(id: ContactId, angle: f64) -> TouchFrame {
    TouchFrame {
        contact: true,
        pos: Some(TouchSample {
            x: 500 + (300.0 * angle.cos()).round() as i32,
            y: 500 + (300.0 * angle.sin()).round() as i32,
        }),
        contact_id: Some(id),
    }
}

fn lift(id: ContactId) -> TouchFrame {
    TouchFrame {
        contact: false,
        pos: None,
        contact_id: Some(id),
    }
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

fn process(
    fsm: &mut Fsm,
    detector: &mut CircularDetector,
    router: &mut Router,
    touch: TouchFrame,
    events: &[InputEvent],
    all_output: &mut Vec<InputEvent>,
) -> Action {
    let previous = fsm.state();
    let tracked_before = fsm.contact_id();
    let action = fsm.step(touch, detector, &Scroll::default());
    let current = fsm.state();
    let capture = if matches!(previous, FsmState::Scrolling { .. }) {
        tracked_before
    } else if matches!(current, FsmState::Scrolling { .. }) {
        fsm.contact_id()
    } else {
        None
    };
    let mut output = Vec::new();
    router.route_frame(
        events,
        capture.map_or(RoutingMode::Passthrough, RoutingMode::Capture),
        &mut output,
    );
    all_output.extend(output);
    action
}

#[test]
fn button_held_engagement_release_and_tracked_lift_preserve_lifecycle() {
    let mut fsm = Fsm::new(500, 500);
    let mut detector = CircularDetector::new();
    let mut router = Router::new();
    let mut output = Vec::new();

    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, 0.0),
        &frame(vec![
            key(Key::BTN_LEFT, 1),
            key(Key::BTN_TOUCH, 1),
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, CONTACT_A.tracking_id),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 800),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 500),
        ]),
        &mut output,
    );
    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, PI / 8.0),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 777),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 615),
        ]),
        &mut output,
    );
    assert!(matches!(fsm.state(), FsmState::Scrolling { .. }));

    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, PI / 4.0),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 712),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 712),
        ]),
        &mut output,
    );
    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, PI / 4.0),
        &frame(vec![key(Key::BTN_LEFT, 0)]),
        &mut output,
    );
    assert!(matches!(fsm.state(), FsmState::Scrolling { .. }));

    process(
        &mut fsm,
        &mut detector,
        &mut router,
        lift(CONTACT_A),
        &frame(vec![
            key(Key::BTN_TOUCH, 0),
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
        ]),
        &mut output,
    );

    assert!(matches!(fsm.state(), FsmState::Debounce));
    let left_values = output
        .iter()
        .filter(|event| event.event_type() == EventType::KEY && event.code() == Key::BTN_LEFT.0)
        .map(InputEvent::value)
        .collect::<Vec<_>>();
    assert_eq!(left_values, vec![1, 0]);
    assert!(output.iter().any(|event| {
        event.event_type() == EventType::ABSOLUTE
            && event.code() == AbsoluteAxisType::ABS_MT_TRACKING_ID.0
            && event.value() == -1
    }));
}

#[test]
fn second_contact_add_move_and_lift_do_not_end_or_steal_session() {
    let contact_b = ContactId {
        slot: 0,
        tracking_id: 10,
    };
    let mut fsm = Fsm::new(500, 500);
    let mut detector = CircularDetector::new();
    let mut router = Router::new();
    let mut output = Vec::new();

    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, 0.0),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, CONTACT_A.tracking_id),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 800),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 500),
        ]),
        &mut output,
    );
    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, PI / 8.0),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 777),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 615),
        ]),
        &mut output,
    );

    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, PI / 8.0),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, contact_b.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, contact_b.tracking_id),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 200),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 300),
        ]),
        &mut output,
    );
    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, PI / 8.0),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 220),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 320),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
        ]),
        &mut output,
    );
    assert_eq!(fsm.contact_id(), Some(CONTACT_A));
    assert!(matches!(fsm.state(), FsmState::Scrolling { .. }));

    process(
        &mut fsm,
        &mut detector,
        &mut router,
        touch(CONTACT_A, PI / 2.0),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_POSITION_X, 500),
            abs(AbsoluteAxisType::ABS_MT_POSITION_Y, 800),
        ]),
        &mut output,
    );
    assert!(matches!(fsm.state(), FsmState::Scrolling { .. }));

    process(
        &mut fsm,
        &mut detector,
        &mut router,
        lift(CONTACT_A),
        &frame(vec![
            abs(AbsoluteAxisType::ABS_MT_SLOT, CONTACT_A.slot as i32),
            abs(AbsoluteAxisType::ABS_MT_TRACKING_ID, -1),
        ]),
        &mut output,
    );
    assert!(matches!(fsm.state(), FsmState::Debounce));

    assert!(output.iter().any(|event| {
        event.event_type() == EventType::ABSOLUTE
            && event.code() == AbsoluteAxisType::ABS_MT_POSITION_X.0
            && event.value() == 220
    }));
    let ended_ids = output
        .iter()
        .filter(|event| {
            event.event_type() == EventType::ABSOLUTE
                && event.code() == AbsoluteAxisType::ABS_MT_TRACKING_ID.0
                && event.value() == -1
        })
        .count();
    assert_eq!(ended_ids, 2);
}
