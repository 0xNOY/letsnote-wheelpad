// Port of FUN_140005bf0 — see analysis/RE-findings.md §6 and analysis/linux-design.md §6.

use std::collections::VecDeque;
use std::f64::consts::PI;

const PI2: f64 = 2.0 * PI;

pub const TRIGGER_ANGLE: f64 = PI / 12.0;
pub const NOISE_REJECT_ANGLE: f64 = PI / 4.0;
pub const ZONE_RADIANS: f64 = PI / 8.0;
pub const SAMPLE_DEADBAND_SQ: i64 = 400;
pub const SENSITIVITY_TABLE: [i32; 5] = [10, 14, 20, 28, 40];

/// Fixed at 20 to match Windows WheelPad exactly (DAT_14003cbec clamp at
/// FUN_1400046a0 lines 65-67). The earlier design (D-021) scaled this
/// from a startup-measured packet rate; hardware testing on the CF-SV2
/// showed that scaling subtly changed scrolling startup feel, so we
/// revert to Windows-faithful behaviour. See DECISIONS.md D-021-followup.
pub const HISTORY_CAPACITY: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchSample {
    pub x: i32,
    pub y: i32,
}

pub struct CircularDetector {
    history: VecDeque<TouchSample>,
    last_stored: Option<TouchSample>,
    accumulator: f64,
}

impl Default for CircularDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CircularDetector {
    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(HISTORY_CAPACITY),
            last_stored: None,
            accumulator: 0.0,
        }
    }

    pub fn on_gesture_start(&mut self) {
        self.history.clear();
        self.last_stored = None;
        self.accumulator = 0.0;
    }

    /// Mirrors FUN_1400046a0 line 62. The 20-unit dead band is computed
    /// against the **previously stored sample**, not the engagement-start
    /// point — this is one of the verified corrections.
    pub fn push_if_moved(&mut self, s: TouchSample) {
        if let Some(prev) = self.last_stored {
            let dx = (s.x - prev.x) as i64;
            let dy = (s.y - prev.y) as i64;
            if dx * dx + dy * dy <= SAMPLE_DEADBAND_SQ {
                return;
            }
        }
        self.last_stored = Some(s);
        if self.history.len() == HISTORY_CAPACITY {
            self.history.pop_back();
        }
        self.history.push_front(s);
    }

    /// FUN_140005bf0 ported with all seven verification-pass corrections
    /// plus the deliberate while-loop drain (D-006). The return value is
    /// the signed wheel-tick count: positive accumulator overflow returns
    /// negative ticks, mirroring the Windows internal sign convention.
    /// The user-facing reverse flip is applied later, in uinput.rs.
    pub fn step(&mut self, scroll_speed_adjust: i32) -> i32 {
        let n = self.history.len();
        if n < 3 {
            return 0;
        }

        // 1. Per-pair motion-vector angles.
        let mut a = Vec::with_capacity(n - 1);
        for i in 0..n - 1 {
            let dx = (self.history[i].x - self.history[i + 1].x) as f64;
            let dy = (self.history[i].y - self.history[i + 1].y) as f64;
            a.push(dy.atan2(dx));
        }

        // 2. Pairwise differences with ±2π wrap and history-truncating
        //    π/4 reject (FUN_140005bf0 lines 80-92).
        let mut sum = 0.0;
        let mut valid = a.len() - 1;
        for i in 0..a.len() - 1 {
            let mut d = a[i] - a[i + 1];
            if d > PI {
                d -= PI2;
            }
            if d < -PI {
                d += PI2;
            }
            if d.abs() > NOISE_REJECT_ANGLE {
                valid = i;
                break;
            }
            sum += d;
        }
        if valid < 3 {
            return 0;
        }

        // 3. Sensitivity-table-weighted mean accumulates into the global.
        let idx = (scroll_speed_adjust.clamp(-2, 2) + 2) as usize;
        let sensitivity = SENSITIVITY_TABLE[idx] as f64;
        self.accumulator += sensitivity * (sum / valid as f64);

        // 4. WHILE-LOOP DRAIN — Linux deviation from Windows. See
        //    DECISIONS.md D-006. Windows FUN_140005bf0 (lines 113-136) is
        //    a single-pass branch that emits at most one tick per packet
        //    and silently loses angle on fast sweeps; we drain fully so
        //    that arbitrarily fast circles still scroll the proportional
        //    amount.
        //
        //    Sign convention preserved from Windows: positive accumulator
        //    overflow yields a tick value of -1. The user-visible
        //    `WheelReverse` flip is applied at the emit layer
        //    (uinput.rs), not here. A clockwise gesture (which integrates
        //    positive in screen-Y-down coords) therefore returns negative
        //    ticks; passing the value through to uinput unchanged scrolls
        //    the page DOWN, matching Windows.
        //
        //    Known quirk preserved as a comment for archaeology:
        //    FUN_140005bf0 line 120 contains a defensive clamp that snaps
        //    the accumulator to -π after the +2π correction rather than
        //    to 0. Our while-loop makes the clamp unreachable, but the
        //    note remains so future readers don't think the quirk was
        //    overlooked.
        let mut ticks: i32 = 0;
        while self.accumulator > PI {
            self.accumulator -= PI2;
            ticks -= 1;
        }
        while self.accumulator < -PI {
            self.accumulator += PI2;
            ticks += 1;
        }
        ticks
    }

    /// Test-only setter. Visible to integration tests under `tests/`.
    #[doc(hidden)]
    pub fn set_accumulator_for_test(&mut self, v: f64) {
        self.accumulator = v;
    }
}

/// Engagement gate — state 3 → state 4 transition test. Center-relative
/// atan2 sweep from the engagement-start point to the current sample,
/// with symmetric ±2π wrap (we deliberately use symmetric form across
/// the daemon — see RE-findings.md §5 footnote on the asymmetric wrap).
pub fn engagement_swept_angle(
    center_x: i32,
    center_y: i32,
    engage_start: TouchSample,
    current: TouchSample,
) -> f64 {
    let ax = (engage_start.x - center_x) as f64;
    let ay = (engage_start.y - center_y) as f64;
    let bx = (current.x - center_x) as f64;
    let by = (current.y - center_y) as f64;
    let mut d = by.atan2(bx) - ay.atan2(ax);
    if d > PI {
        d -= PI2;
    }
    if d < -PI {
        d += PI2;
    }
    d
}

/// Radial gate from FUN_140005a00 line 31 and FUN_1400046a0 lines 129/187.
/// Returns true if the centered sample is in the outer ring (i.e., outside
/// the inner dead-zone radius).
pub fn radial_gate_ok(
    center_x: i32,
    center_y: i32,
    s: TouchSample,
    detect_area_width: i32,
    detect_area_radius: f64,
    detect_area_aspect_ratio: f64,
) -> bool {
    let dx = (s.x - center_x) as f64;
    let dy = (s.y - center_y) as f64 * detect_area_aspect_ratio;
    let width_fraction = (10 - detect_area_width.clamp(0, 10)) as f64 / 10.0;
    let inner_radius = detect_area_radius * width_fraction;
    dx * dx + dy * dy >= inner_radius * inner_radius
}

/// Horizontal-arc test (FUN_140005a00 lines 65-74). Returns true if the
/// centered sample's atan2 lies within the configured wedge. Wraparound
/// (`start > end`) is handled by splitting the test. Caller guarantees
/// `horizontal_enable = true`; this function MUST NOT be called when
/// horizontal scrolling is disabled (see linux-design.md §5 "Vertical
/// scroll is NOT angle-gated").
pub fn within_horizontal_arc(
    center_x: i32,
    center_y: i32,
    s: TouchSample,
    horizontal_start: i32,
    horizontal_end: i32,
) -> bool {
    let dx = (s.x - center_x) as f64;
    let dy = (s.y - center_y) as f64;
    let mut theta = dy.atan2(dx);
    if theta < 0.0 {
        theta += PI2;
    }
    let start = horizontal_start as f64 * ZONE_RADIANS;
    let end = horizontal_end as f64 * ZONE_RADIANS;
    if start <= end {
        theta >= start && theta <= end
    } else {
        // Wedge wraps across 2π.
        theta >= start || theta <= end
    }
}
