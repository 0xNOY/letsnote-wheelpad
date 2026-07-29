// FSM synthetic tests — see linux-design.md §13.

use std::f64::consts::PI;

use letsnote_wheelpad::config::Scroll;
use letsnote_wheelpad::detector::{CircularDetector, CoordinateTransform, TouchSample};
use letsnote_wheelpad::fsm::{Action, Fsm, FsmState, TouchFrame};

fn default_scroll() -> Scroll {
    Scroll::default()
}

fn lift() -> TouchFrame {
    TouchFrame {
        contact: false,
        pos: None,
    }
}

fn touch(x: i32, y: i32) -> TouchFrame {
    TouchFrame {
        contact: true,
        pos: Some(TouchSample { x, y }),
    }
}

fn physical_touch(
    center_x: i32,
    center_y: i32,
    radius: f64,
    angle: f64,
    y_scale: f64,
) -> TouchFrame {
    touch(
        center_x + (radius * angle.cos()).round() as i32,
        center_y + (radius * angle.sin() / y_scale).round() as i32,
    )
}

fn drive(
    fsm: &mut Fsm,
    detector: &mut CircularDetector,
    scroll: &Scroll,
    frames: &[TouchFrame],
) -> Vec<Action> {
    let mut acc = Vec::new();
    for f in frames {
        let action = fsm.step(*f, detector, scroll);
        if !matches!(action, Action::None) {
            acc.push(action);
        }
    }
    acc
}

#[test]
fn idle_to_contact_on_touchdown_inside_dead_zone() {
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();
    let _ = drive(&mut fsm, &mut det, &scroll, &[touch(510, 510)]);
    assert!(matches!(fsm.state(), FsmState::Contact { .. }));
}

#[test]
fn idle_to_moving_on_touchdown_outside_dead_zone() {
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();
    let _ = drive(&mut fsm, &mut det, &scroll, &[touch(720, 500)]); // r = 220
    assert!(matches!(fsm.state(), FsmState::Moving { .. }));
}

#[test]
fn contact_to_idle_on_lift() {
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();
    drive(&mut fsm, &mut det, &scroll, &[touch(510, 510), lift()]);
    assert!(matches!(fsm.state(), FsmState::Idle));
}

#[test]
fn contact_does_not_engage_on_cross_gate_movement_d020() {
    // D-020: strict Windows dead-zone semantics. Once trapped in
    // Contact, even sliding outside the gate does not engage Moving.
    // The user must lift and re-touch.
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();
    // Touch inside dead zone, then slide outside.
    drive(
        &mut fsm,
        &mut det,
        &scroll,
        &[touch(510, 510), touch(550, 500), touch(720, 500)],
    );
    assert!(
        matches!(fsm.state(), FsmState::Contact { .. }),
        "expected to stay in Contact, got {:?}",
        fsm.state()
    );
}

#[test]
fn moving_to_idle_on_lift_before_engagement() {
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();
    drive(&mut fsm, &mut det, &scroll, &[touch(720, 500), lift()]);
    assert!(matches!(fsm.state(), FsmState::Idle));
}

#[test]
fn moving_to_contact_on_slip_back_into_dead_zone() {
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();
    // Engage outside, then slip back inside.
    drive(
        &mut fsm,
        &mut det,
        &scroll,
        &[touch(720, 500), touch(550, 500)],
    );
    assert!(matches!(fsm.state(), FsmState::Contact { .. }));
}

#[test]
fn moving_to_scrolling_on_swept_angle_past_trigger() {
    // Sweep > π/12 from engage_start while staying outside the radial
    // gate → Scrolling. With the passthrough architecture there is no
    // longer a Grab action to observe; the state transition is the
    // signal the runtime keys off.
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();

    // engage_start at angle 0, r=220
    let start = touch(720, 500);
    // sweep to angle π/8 (= 22.5°, > π/12 = 15°), r=220
    let theta = PI / 8.0;
    let end_x = 500 + (220.0 * theta.cos()).round() as i32;
    let end_y = 500 + (220.0 * theta.sin()).round() as i32;
    let end = touch(end_x, end_y);

    drive(&mut fsm, &mut det, &scroll, &[start, end]);
    assert!(matches!(fsm.state(), FsmState::Scrolling));
}

#[test]
fn anisotropic_coordinates_preserve_engagement_threshold_around_both_axes() {
    const Y_SCALE: f64 = 4570.0 / 3430.0;
    const CENTER: i32 = 5000;
    let transform = CoordinateTransform::new(Y_SCALE);
    let mut scroll = default_scroll();
    scroll.detect_area_radius = 1500.0;
    scroll.coordinate_y_scale = Y_SCALE;

    for start_angle in [0.0, PI / 2.0] {
        let mut fsm = Fsm::with_transform(CENTER, CENTER, transform);
        let mut detector = CircularDetector::with_transform(transform);
        let start = physical_touch(CENTER, CENTER, 2000.0, start_angle, Y_SCALE);
        let below = physical_touch(
            CENTER,
            CENTER,
            2000.0,
            start_angle + 14.0_f64.to_radians(),
            Y_SCALE,
        );
        let above = physical_touch(
            CENTER,
            CENTER,
            2000.0,
            start_angle + 16.0_f64.to_radians(),
            Y_SCALE,
        );

        drive(&mut fsm, &mut detector, &scroll, &[start, below]);
        assert!(
            matches!(fsm.state(), FsmState::Moving { .. }),
            "14° should remain below the trigger near angle {start_angle}"
        );

        drive(&mut fsm, &mut detector, &scroll, &[above]);
        assert!(
            matches!(fsm.state(), FsmState::Scrolling),
            "16° should cross the trigger near angle {start_angle}"
        );
    }
}

#[test]
fn anisotropic_physical_circle_matches_isotropic_fsm_ticks() {
    const Y_SCALE: f64 = 4570.0 / 3430.0;
    const CENTER: i32 = 5000;
    let mut scroll = default_scroll();
    scroll.detect_area_radius = 1500.0;

    let frames_for = |y_scale: f64| {
        (0..=40)
            .map(|i| physical_touch(CENTER, CENTER, 2000.0, 2.0 * PI * i as f64 / 40.0, y_scale))
            .collect::<Vec<_>>()
    };
    let tick_sum = |frames: &[TouchFrame], transform: CoordinateTransform, scroll: &Scroll| {
        let mut fsm = Fsm::with_transform(CENTER, CENTER, transform);
        let mut detector = CircularDetector::with_transform(transform);
        drive(&mut fsm, &mut detector, scroll, frames)
            .into_iter()
            .map(|action| match action {
                Action::EmitWheelV(ticks) | Action::EmitWheelH(ticks) => ticks,
                Action::None => 0,
            })
            .sum::<i32>()
    };

    let expected = tick_sum(&frames_for(1.0), CoordinateTransform::default(), &scroll);
    scroll.coordinate_y_scale = Y_SCALE;
    let actual = tick_sum(
        &frames_for(Y_SCALE),
        CoordinateTransform::new(Y_SCALE),
        &scroll,
    );

    assert_ne!(expected, 0);
    assert_eq!(actual.signum(), expected.signum());
    assert!(
        (actual - expected).abs() <= 1,
        "anisotropic={actual}, isotropic={expected}"
    );
}

#[test]
fn scrolling_to_debounce_on_lift() {
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();

    let start = touch(720, 500);
    let theta = PI / 8.0;
    let mid_x = 500 + (220.0 * theta.cos()).round() as i32;
    let mid_y = 500 + (220.0 * theta.sin()).round() as i32;
    let mid = touch(mid_x, mid_y);

    drive(&mut fsm, &mut det, &scroll, &[start, mid]);
    assert!(matches!(fsm.state(), FsmState::Scrolling));

    drive(&mut fsm, &mut det, &scroll, &[lift()]);
    assert!(matches!(fsm.state(), FsmState::Debounce));
}

#[test]
fn force_idle_resets_state() {
    // Watchdog path: after force_idle the FSM is back at Idle even if
    // it was mid-Scrolling.
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();

    let start = touch(720, 500);
    let theta = PI / 8.0;
    let mid = touch(
        500 + (220.0 * theta.cos()).round() as i32,
        500 + (220.0 * theta.sin()).round() as i32,
    );
    drive(&mut fsm, &mut det, &scroll, &[start, mid]);
    assert!(matches!(fsm.state(), FsmState::Scrolling));

    fsm.force_idle(&mut det);
    assert!(matches!(fsm.state(), FsmState::Idle));
}

#[test]
fn debounce_to_idle_on_next_frame_no_timer() {
    // D-011-followup: Debounce always exits to Idle on the next frame
    // regardless of whether the finger is now down or up.
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();

    let start = touch(720, 500);
    let theta = PI / 8.0;
    let mid_x = 500 + (220.0 * theta.cos()).round() as i32;
    let mid_y = 500 + (220.0 * theta.sin()).round() as i32;
    let mid = touch(mid_x, mid_y);

    drive(&mut fsm, &mut det, &scroll, &[start, mid, lift()]);
    assert!(matches!(fsm.state(), FsmState::Debounce));

    drive(&mut fsm, &mut det, &scroll, &[lift()]);
    assert!(matches!(fsm.state(), FsmState::Idle));
}

#[test]
fn debounce_to_idle_even_if_finger_back_down() {
    // Per D-011-followup, the Debounce state has no re-engagement
    // path. If a finger is down on the very next frame, we still go to
    // Idle this frame; the frame *after* that re-runs the fresh-touch
    // classifier.
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let scroll = default_scroll();

    let start = touch(720, 500);
    let theta = PI / 8.0;
    let mid_x = 500 + (220.0 * theta.cos()).round() as i32;
    let mid_y = 500 + (220.0 * theta.sin()).round() as i32;
    let mid = touch(mid_x, mid_y);

    drive(&mut fsm, &mut det, &scroll, &[start, mid, lift()]);
    assert!(matches!(fsm.state(), FsmState::Debounce));

    drive(&mut fsm, &mut det, &scroll, &[touch(720, 500)]);
    assert!(matches!(fsm.state(), FsmState::Idle));
}

#[test]
fn disabled_scroll_holds_idle() {
    // D-007: when scroll.enable = false the daemon keeps reading
    // frames but the FSM never advances past Idle.
    let mut fsm = Fsm::new(500, 500);
    let mut det = CircularDetector::new();
    let mut scroll = default_scroll();
    scroll.enable = false;
    drive(
        &mut fsm,
        &mut det,
        &scroll,
        &[touch(720, 500), touch(700, 520), lift()],
    );
    assert!(matches!(fsm.state(), FsmState::Idle));
}
