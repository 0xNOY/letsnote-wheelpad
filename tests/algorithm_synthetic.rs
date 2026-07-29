// Synthetic tests for the CircularDetector — see linux-design.md §13.

use std::f64::consts::PI;

use letsnote_wheelpad::detector::{
    engagement_swept_angle, radial_gate_ok, within_horizontal_arc, CircularDetector,
    CoordinateTransform, TouchSample, TRIGGER_ANGLE,
};

const IDENTITY: CoordinateTransform = CoordinateTransform::new(1.0);

/// Generate N samples around a circle. Y is screen-down, so a positive
/// sweep is clockwise in screen space (consistent with WheelPad's
/// internal sign convention).
fn circle_samples(
    center_x: i32,
    center_y: i32,
    r: f64,
    start_rad: f64,
    total_sweep_rad: f64,
    n: usize,
) -> Vec<TouchSample> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / (n - 1).max(1) as f64;
        let theta = start_rad + total_sweep_rad * t;
        let x = center_x + (r * theta.cos()).round() as i32;
        let y = center_y + (r * theta.sin()).round() as i32;
        out.push(TouchSample { x, y });
    }
    out
}

fn anisotropic_circle_samples(
    center_x: i32,
    center_y: i32,
    r: f64,
    start_rad: f64,
    total_sweep_rad: f64,
    n: usize,
    y_scale: f64,
) -> Vec<TouchSample> {
    circle_samples(center_x, center_y, r, start_rad, total_sweep_rad, n)
        .into_iter()
        .map(|s| TouchSample {
            x: s.x,
            y: center_y + ((s.y - center_y) as f64 / y_scale).round() as i32,
        })
        .collect()
}

fn run_gesture(samples: &[TouchSample], sensitivity: i32) -> i32 {
    run_gesture_with_transform(samples, sensitivity, IDENTITY)
}

fn run_gesture_with_transform(
    samples: &[TouchSample],
    sensitivity: i32,
    transform: CoordinateTransform,
) -> i32 {
    let mut d = CircularDetector::with_transform(transform);
    d.on_gesture_start();
    let mut total = 0_i32;
    for s in samples {
        d.push_if_moved(*s);
        total += d.step(sensitivity);
    }
    total
}

#[test]
fn full_clockwise_circle_emits_negative_ticks() {
    // A clockwise circle in screen-Y-down integrates positive radians;
    // positive overflow returns negative ticks (Windows internal sign
    // convention). Passing the value through to uinput unchanged
    // scrolls DOWN — matching the worked example in linux-design.md §6.
    let samples = circle_samples(500, 500, 200.0, 0.0, 2.0 * PI, 40);
    let total = run_gesture(&samples, 0);
    assert!(
        total < 0,
        "clockwise circle should produce negative ticks (got {total})"
    );
    assert!(
        total.abs() >= 1,
        "full circle should produce at least one tick"
    );
}

#[test]
fn full_counterclockwise_circle_emits_positive_ticks() {
    let samples = circle_samples(500, 500, 200.0, 0.0, -2.0 * PI, 40);
    let total = run_gesture(&samples, 0);
    assert!(
        total > 0,
        "counterclockwise circle should produce positive ticks (got {total})"
    );
}

#[test]
fn straight_line_produces_zero_ticks() {
    let samples: Vec<_> = (0..20)
        .map(|i| TouchSample {
            x: 100 + i * 30,
            y: 500,
        })
        .collect();
    let total = run_gesture(&samples, 0);
    assert_eq!(total, 0, "straight line must not scroll");
}

#[test]
fn zig_zag_does_not_engage() {
    // Alternating sign deltas larger than π/4 → per-delta π/4 reject
    // truncates history below 3 valid deltas, so step returns 0
    // every packet.
    let zigs: Vec<TouchSample> = [
        (100, 100),
        (130, 200),
        (160, 100),
        (190, 200),
        (220, 100),
        (250, 200),
        (280, 100),
        (310, 200),
        (340, 100),
        (370, 200),
        (400, 100),
        (430, 200),
        (460, 100),
        (490, 200),
        (520, 100),
        (550, 200),
        (580, 100),
        (610, 200),
        (640, 100),
        (670, 200),
    ]
    .iter()
    .map(|(x, y)| TouchSample { x: *x, y: *y })
    .collect();
    let total = run_gesture(&zigs, 0);
    assert_eq!(total, 0, "zig-zag should not produce ticks (got {total})");
}

#[test]
fn half_circle_does_not_exceed_full() {
    let full = run_gesture(&circle_samples(500, 500, 200.0, 0.0, 2.0 * PI, 40), 0);
    let half = run_gesture(&circle_samples(500, 500, 200.0, 0.0, PI, 20), 0);
    assert!(full < 0 && half < 0);
    assert!(
        half.abs() <= full.abs(),
        "half ({half}) should not exceed full ({full})"
    );
}

#[test]
fn reverse_circle_has_opposite_sign() {
    let cw = run_gesture(&circle_samples(500, 500, 200.0, 0.0, 2.0 * PI, 40), 0);
    let ccw = run_gesture(&circle_samples(500, 500, 200.0, 0.0, -2.0 * PI, 40), 0);
    assert!(cw < 0 && ccw > 0, "cw={cw} ccw={ccw}");
}

#[test]
fn anisotropic_raw_circle_matches_isotropic_detector_output() {
    const Y_SCALE: f64 = 4570.0 / 3430.0;
    let isotropic = circle_samples(3500, 3000, 1900.0, 0.0, 2.0 * PI, 40);
    let anisotropic = anisotropic_circle_samples(3500, 3000, 1900.0, 0.0, 2.0 * PI, 40, Y_SCALE);

    let expected = run_gesture(&isotropic, 0);
    let actual = run_gesture_with_transform(&anisotropic, 0, CoordinateTransform::new(Y_SCALE));
    assert_eq!(actual, expected);
}

#[test]
fn sign_convention_positive_overflow_yields_negative_tick() {
    // Populate history, then manually push the accumulator past +π and
    // verify a subsequent step drains negative. This sidesteps any
    // dependence on real gesture geometry; it directly exercises the
    // emit branch.
    let mut d = CircularDetector::new();
    d.on_gesture_start();
    for s in circle_samples(500, 500, 200.0, 0.0, 0.5, 5) {
        d.push_if_moved(s);
        let _ = d.step(0);
    }
    d.set_accumulator_for_test(PI + 0.01);
    let ticks = d.step(0);
    assert_eq!(ticks, -1);
}

#[test]
fn engagement_swept_angle_is_signed() {
    let start = TouchSample { x: 200, y: 0 };
    let end = TouchSample {
        x: (200.0 * (PI / 4.0).cos()).round() as i32,
        y: (200.0 * (PI / 4.0).sin()).round() as i32,
    };
    let swept = engagement_swept_angle(0, 0, start, end, IDENTITY);
    assert!(swept > 0.0);
    assert!((swept - PI / 4.0).abs() < 0.01);

    let swept_rev = engagement_swept_angle(0, 0, end, start, IDENTITY);
    assert!(swept_rev < 0.0);
}

#[test]
fn engagement_threshold_pi_over_12() {
    // Use r = 5000 instead of 200 so i32 rounding error
    // (arctan(0.5 / r)) is well below the ±0.001 rad margin we use to
    // straddle the trigger. At r = 200 the rounding error was ~0.0025
    // rad, larger than the margin, which made the boundary test
    // flaky.
    const R: f64 = 5000.0;
    let start = TouchSample { x: R as i32, y: 0 };

    // Just past π/12 (15°) → above trigger.
    let theta = TRIGGER_ANGLE + 0.001;
    let end_above = TouchSample {
        x: (R * theta.cos()).round() as i32,
        y: (R * theta.sin()).round() as i32,
    };
    assert!(engagement_swept_angle(0, 0, start, end_above, IDENTITY).abs() > TRIGGER_ANGLE);

    // Just shy of π/12 → below trigger.
    let theta = TRIGGER_ANGLE - 0.001;
    let end_below = TouchSample {
        x: (R * theta.cos()).round() as i32,
        y: (R * theta.sin()).round() as i32,
    };
    assert!(engagement_swept_angle(0, 0, start, end_below, IDENTITY).abs() < TRIGGER_ANGLE);
}

#[test]
fn radial_gate_default_width_requires_outer_ring() {
    // DetectAreaWidth = 0 → r ≥ 200 units from center.
    assert!(!radial_gate_ok(
        500,
        500,
        TouchSample { x: 500, y: 500 },
        0,
        200.0,
        IDENTITY
    ));
    assert!(!radial_gate_ok(
        500,
        500,
        TouchSample { x: 600, y: 500 },
        0,
        200.0,
        IDENTITY
    )); // r = 100
    assert!(radial_gate_ok(
        500,
        500,
        TouchSample { x: 700, y: 500 },
        0,
        200.0,
        IDENTITY
    )); // r = 200
    assert!(radial_gate_ok(
        500,
        500,
        TouchSample { x: 800, y: 500 },
        0,
        200.0,
        IDENTITY
    )); // r = 300
}

#[test]
fn radial_gate_max_width_engages_anywhere() {
    // DetectAreaWidth = 10 → r ≥ 0 (whole pad active).
    assert!(radial_gate_ok(
        500,
        500,
        TouchSample { x: 500, y: 500 },
        10,
        200.0,
        IDENTITY
    ));
    assert!(radial_gate_ok(
        500,
        500,
        TouchSample { x: 600, y: 500 },
        10,
        200.0,
        IDENTITY
    ));
}

#[test]
fn radial_gate_supports_high_resolution_anisotropic_coordinates() {
    // A circular pad can report different X/Y coordinate densities.
    // Scaling Y by the measured X-range / Y-range makes these equal
    // physical distances equivalent.
    let transform = CoordinateTransform::new(4570.0 / 3430.0);
    assert!(radial_gate_ok(
        3495,
        2965,
        TouchSample { x: 5460, y: 2965 },
        0,
        1965.0,
        transform
    ));
    assert!(radial_gate_ok(
        3495,
        2965,
        TouchSample { x: 3495, y: 4441 },
        0,
        1965.0,
        transform
    ));
    assert!(!radial_gate_ok(
        3495,
        2965,
        TouchSample { x: 5000, y: 2965 },
        0,
        1965.0,
        transform
    ));
}

#[test]
fn radial_gate_defaults_match_legacy_formula_for_all_widths() {
    for width in 0..=10 {
        for dx in (-300..=300).step_by(25) {
            for dy in (-300..=300).step_by(25) {
                let expected = {
                    let r2 = i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy);
                    let w = i64::from(10 - width);
                    r2 >= w * w * 400
                };
                let actual = radial_gate_ok(
                    500,
                    500,
                    TouchSample {
                        x: 500 + dx,
                        y: 500 + dy,
                    },
                    width,
                    200.0,
                    IDENTITY,
                );
                assert_eq!(actual, expected, "width={width}, dx={dx}, dy={dy}");
            }
        }
    }
}

#[test]
fn horizontal_arc_default_45_to_135_is_bottom_edge() {
    // Defaults: horizontal_start=2 → 45°, horizontal_end=6 → 135°.
    // In screen-Y-down, the wedge 45°→90°→135° is the BOTTOM half-rim.
    let south = TouchSample { x: 500, y: 700 }; // dy=+200 → 90°
    let north = TouchSample { x: 500, y: 300 }; // dy=-200 → -90° = 270°
    let east = TouchSample { x: 700, y: 500 };
    let west = TouchSample { x: 300, y: 500 };
    assert!(within_horizontal_arc(500, 500, south, 2, 6, IDENTITY));
    assert!(!within_horizontal_arc(500, 500, north, 2, 6, IDENTITY));
    assert!(!within_horizontal_arc(500, 500, east, 2, 6, IDENTITY));
    assert!(!within_horizontal_arc(500, 500, west, 2, 6, IDENTITY));
}

#[test]
fn horizontal_arc_wraparound() {
    // start > end → arc wraps across 0°. start=14 (315°), end=2 (45°)
    // → 315°..360° + 0°..45°.
    let east = TouchSample { x: 700, y: 500 }; // 0°
    let southeast = TouchSample {
        x: 500 + (200.0_f64 * (PI / 4.0).cos()).round() as i32,
        y: 500 + (200.0_f64 * (PI / 4.0).sin()).round() as i32,
    }; // 45°
    let southwest = TouchSample {
        x: 500 + (200.0_f64 * (3.0 * PI / 4.0).cos()).round() as i32,
        y: 500 + (200.0_f64 * (3.0 * PI / 4.0).sin()).round() as i32,
    }; // 135°
    assert!(within_horizontal_arc(500, 500, east, 14, 2, IDENTITY));
    assert!(within_horizontal_arc(500, 500, southeast, 14, 2, IDENTITY));
    assert!(!within_horizontal_arc(500, 500, southwest, 14, 2, IDENTITY));
}

#[test]
fn horizontal_arc_uses_transformed_physical_angles() {
    const Y_SCALE: f64 = 4570.0 / 3430.0;
    let transform = CoordinateTransform::new(Y_SCALE);
    let at_45 = TouchSample { x: 9570, y: 8430 };
    let at_90 = TouchSample { x: 5000, y: 8430 };
    let at_135 = TouchSample { x: 430, y: 8430 };
    let sample_at = |degrees: f64| {
        let theta = degrees * PI / 180.0;
        TouchSample {
            x: 5000 + (2000.0 * theta.cos()).round() as i32,
            y: 5000 + (2000.0 * theta.sin() / Y_SCALE).round() as i32,
        }
    };

    assert!(within_horizontal_arc(5000, 5000, at_45, 2, 6, transform,));
    assert!(within_horizontal_arc(5000, 5000, at_90, 2, 6, transform,));
    assert!(within_horizontal_arc(5000, 5000, at_135, 2, 6, transform,));
    assert!(!within_horizontal_arc(
        5000,
        5000,
        sample_at(40.0),
        2,
        6,
        transform,
    ));
    assert!(!within_horizontal_arc(
        5000,
        5000,
        sample_at(140.0),
        2,
        6,
        transform,
    ));
}
