//! One automation lane.
//!
//! Breakpoints are sample time and value. `curve` and `heard` do not allocate.
//! Edits may grow the point list. Record spacing is 64 samples.

const RECORD_SPACING: u64 = 64;

/// How the lane chooses what is heard and what a gesture stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationMode {
    Manual,
    Play,
    Write,
    Touch,
    Latch,
}

/// How [`AutomationLane::curve`] joins neighboring breakpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomationInterpolation {
    Linear,
    Discrete,
}

/// Same five modes. This name is the one the integration tests use.
pub type LaneMode = AutomationMode;

/// Same two interpolations. This name is the one the integration tests use.
pub type CurveInterpolation = AutomationInterpolation;

/// Limits, hand, heard value, and an ordered breakpoint list.
#[derive(Clone, Debug)]
pub struct AutomationLane {
    lower: f32,
    upper: f32,
    initial: f32,
    hand: f32,
    heard: f32,
    mode: AutomationMode,
    interpolation: AutomationInterpolation,
    touch_down: bool,
    rolling: bool,
    /// A Latch gesture was accepted during the current rolling pass.
    latch_hold: bool,
    playhead: u64,
    points: Vec<(u64, f32)>,
}

impl AutomationLane {
    /// Limits are inclusive. If `lower > upper`, they are swapped.
    /// Hand and heard start at `initial` clamped into those limits.
    pub fn new(lower: f32, upper: f32, initial: f32) -> Self {
        let (lower, upper) = if lower > upper {
            (upper, lower)
        } else {
            (lower, upper)
        };
        let initial = clamp_limit(lower, upper, initial);
        Self {
            lower,
            upper,
            initial,
            hand: initial,
            heard: initial,
            mode: AutomationMode::Manual,
            interpolation: AutomationInterpolation::Linear,
            touch_down: false,
            rolling: false,
            latch_hold: false,
            playhead: 0,
            points: Vec::new(),
        }
    }

    pub fn lower(&self) -> f32 {
        self.lower
    }

    pub fn upper(&self) -> f32 {
        self.upper
    }

    pub fn mode(&self) -> AutomationMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: AutomationMode) {
        self.mode = mode;
    }

    pub fn interpolation(&self) -> AutomationInterpolation {
        self.interpolation
    }

    /// Changes evaluation only. Points stay where they are.
    pub fn set_interpolation(&mut self, interpolation: AutomationInterpolation) {
        self.interpolation = interpolation;
    }

    pub fn hand(&self) -> f32 {
        self.hand
    }

    /// Sanitizes `value`, then lets the mode hear or store it.
    /// NaN, Play, and Touch with the touch up change nothing.
    pub fn set_hand(&mut self, value: f32) {
        let Some(value) = self.sanitize(value) else {
            return;
        };
        if self.gesture_ignored() {
            return;
        }
        self.hand = value;
        match self.mode {
            AutomationMode::Manual => self.heard = value,
            AutomationMode::Play => {}
            AutomationMode::Write => {
                if self.rolling {
                    self.punch(value);
                }
                self.heard = value;
            }
            AutomationMode::Touch => {
                if self.rolling {
                    self.punch(value);
                } else {
                    self.replace_point_at(self.playhead, value);
                }
                self.heard = value;
            }
            AutomationMode::Latch => {
                if self.rolling {
                    self.punch(value);
                    self.latch_hold = true;
                    self.heard = value;
                }
            }
        }
    }

    pub fn heard(&self) -> f32 {
        self.heard
    }

    /// Breakpoint curve at `sample`. Does not read the hand and does not allocate.
    pub fn curve(&self, sample: u64) -> f32 {
        let points = self.points.as_slice();
        if points.is_empty() {
            return self.initial;
        }
        let index = points.partition_point(|point| point.0 < sample);
        if index == 0 {
            return points[0].1;
        }
        if index == points.len() {
            return points[points.len() - 1].1;
        }
        let (start, start_value) = points[index - 1];
        let (end, end_value) = points[index];
        if sample == end || self.interpolation == AutomationInterpolation::Discrete {
            return if sample == end {
                end_value
            } else {
                start_value
            };
        }
        let span = (end - start) as f32;
        if span == 0.0 {
            return end_value;
        }
        let along = (sample - start) as f32;
        start_value + (end_value - start_value) * (along / span)
    }

    pub fn touch(&mut self, down: bool) {
        self.touch_down = down;
        if self.rolling && self.mode == AutomationMode::Touch {
            self.hear_transport();
        }
    }

    pub fn touching(&self) -> bool {
        self.touch_down
    }

    /// `false` stops the pass. Write becomes Touch, and a released curve is heard.
    pub fn set_rolling(&mut self, rolling: bool) {
        if rolling == self.rolling {
            return;
        }
        if rolling {
            self.rolling = true;
            self.latch_hold = false;
            self.hear_transport();
            return;
        }
        self.rolling = false;
        if self.mode == AutomationMode::Write {
            self.mode = AutomationMode::Touch;
        }
        self.latch_hold = false;
        if !self.touch_down && self.hears_curve_at_rest() {
            self.heard = self.curve(self.playhead);
        }
    }

    pub fn rolling(&self) -> bool {
        self.rolling
    }

    pub fn is_rolling(&self) -> bool {
        self.rolling
    }

    /// Place or replace one breakpoint. This is an edit, not a recorded gesture.
    pub fn insert_point(&mut self, sample: u64, value: f32) {
        self.upsert(sample, value);
    }

    pub fn clear_points(&mut self) {
        self.points.clear();
    }

    pub fn set_playhead(&mut self, sample: u64) {
        self.playhead = sample;
        if self.rolling {
            self.hear_transport();
        } else if self.mode == AutomationMode::Play {
            self.heard = self.curve(sample);
        }
    }

    pub fn playhead(&self) -> u64 {
        self.playhead
    }

    pub fn points(&self) -> &[(u64, f32)] {
        &self.points
    }

    fn sanitize(&self, value: f32) -> Option<f32> {
        if value.is_nan() {
            None
        } else if value < self.lower {
            Some(self.lower)
        } else if value > self.upper {
            Some(self.upper)
        } else {
            Some(value)
        }
    }

    fn gesture_ignored(&self) -> bool {
        match self.mode {
            AutomationMode::Play => true,
            AutomationMode::Touch => !self.touch_down,
            AutomationMode::Manual | AutomationMode::Write | AutomationMode::Latch => false,
        }
    }

    fn hears_curve_at_rest(&self) -> bool {
        matches!(
            self.mode,
            AutomationMode::Play | AutomationMode::Touch | AutomationMode::Latch
        )
    }

    fn hear_transport(&mut self) {
        self.heard = match self.mode {
            AutomationMode::Manual | AutomationMode::Write => self.hand,
            AutomationMode::Play => self.curve(self.playhead),
            AutomationMode::Touch => {
                if self.touch_down {
                    self.hand
                } else {
                    self.curve(self.playhead)
                }
            }
            AutomationMode::Latch => {
                if self.latch_hold {
                    self.hand
                } else {
                    self.curve(self.playhead)
                }
            }
        };
    }

    /// Triplet from the curve as it was before any of the three writes.
    fn punch(&mut self, value: f32) {
        let t = self.playhead;
        let later = t.saturating_add(RECORD_SPACING);
        if !self.points.iter().any(|point| point.0 > later) {
            return;
        }
        let restore = self
            .points
            .iter()
            .find(|point| point.0 > t)
            .map(|point| point.1);
        let earlier = if t >= RECORD_SPACING {
            Some(self.curve(t - RECORD_SPACING))
        } else {
            None
        };
        let Some(restore) = restore else {
            return;
        };
        if let Some(sampled) = earlier {
            self.upsert(t - RECORD_SPACING, sampled);
        }
        self.upsert(t, value);
        self.upsert(later, restore);
    }

    fn replace_point_at(&mut self, sample: u64, value: f32) {
        if let Ok(index) = self.points.binary_search_by_key(&sample, |point| point.0) {
            self.points[index].1 = value;
        }
    }

    fn upsert(&mut self, sample: u64, value: f32) {
        match self.points.binary_search_by_key(&sample, |point| point.0) {
            Ok(index) => self.points[index].1 = value,
            Err(index) => self.points.insert(index, (sample, value)),
        }
    }
}

fn clamp_limit(lower: f32, upper: f32, value: f32) -> f32 {
    if value.is_nan() || value < lower {
        lower
    } else if value > upper {
        upper
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(got: f32, expect: f32) {
        assert!(
            (got - expect).abs() <= 1e-4,
            "{got} is not within 1e-4 of {expect}"
        );
    }

    fn point(lane: &AutomationLane, sample: u64) -> f32 {
        lane.points()
            .iter()
            .find(|point| point.0 == sample)
            .map(|point| point.1)
            .unwrap_or_else(|| panic!("no point at {sample}"))
    }

    #[test]
    fn acceptance_01_new_lane_defaults_and_clamped_initial() {
        let lane = AutomationLane::new(0.0, 2.0, 1.0);
        assert_eq!(lane.mode(), AutomationMode::Manual);
        assert_eq!(lane.interpolation(), AutomationInterpolation::Linear);
        assert!(!lane.touching());
        assert!(!lane.rolling());
        assert_eq!(lane.playhead(), 0);
        assert!(lane.points().is_empty());
        approx(lane.hand(), 1.0);
        approx(lane.heard(), 1.0);

        let high = AutomationLane::new(0.0, 2.0, 9.0);
        approx(high.hand(), 2.0);
        approx(high.heard(), 2.0);
        let low = AutomationLane::new(0.0, 2.0, -4.0);
        approx(low.hand(), 0.0);
        approx(low.heard(), 0.0);

        let swapped_high = AutomationLane::new(2.0, 0.0, 9.0);
        approx(swapped_high.hand(), 2.0);
        approx(swapped_high.heard(), 2.0);
        let swapped_low = AutomationLane::new(2.0, 0.0, -4.0);
        approx(swapped_low.hand(), 0.0);
        approx(swapped_low.heard(), 0.0);
        let swapped_mid = AutomationLane::new(2.0, 0.0, 0.5);
        approx(swapped_mid.hand(), 0.5);
        approx(swapped_mid.heard(), 0.5);
    }

    #[test]
    fn acceptance_02_empty_curve_stays_at_initial() {
        let mut lane = AutomationLane::new(0.0, 2.0, 1.0);
        lane.set_hand(0.25);
        approx(lane.curve(0), 1.0);
        approx(lane.curve(500), 1.0);
        approx(lane.hand(), 0.25);
        assert!(lane.points().is_empty());
    }

    #[test]
    fn acceptance_03_hand_sanitizing() {
        let mut lane = AutomationLane::new(0.0, 2.0, 1.0);
        assert_eq!(lane.mode(), AutomationMode::Manual);

        lane.set_hand(3.0);
        approx(lane.heard(), 2.0);
        approx(lane.hand(), 2.0);
        lane.set_hand(-1.0);
        approx(lane.heard(), 0.0);
        approx(lane.hand(), 0.0);
        lane.set_hand(f32::INFINITY);
        approx(lane.heard(), 2.0);
        approx(lane.hand(), 2.0);
        lane.set_hand(f32::NEG_INFINITY);
        approx(lane.heard(), 0.0);
        approx(lane.hand(), 0.0);
        lane.set_hand(f32::NAN);
        approx(lane.heard(), 0.0);
        approx(lane.hand(), 0.0);
        assert!(lane.points().is_empty());
        assert_eq!(lane.mode(), AutomationMode::Manual);

        lane.set_hand(0.0);
        approx(lane.heard(), 0.0);
        approx(lane.hand(), 0.0);
        lane.set_hand(2.0);
        approx(lane.heard(), 2.0);
        approx(lane.hand(), 2.0);
    }

    #[test]
    fn acceptance_04_linear_and_discrete_reference() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
        lane.points = vec![(0, 0.20), (1000, 0.80), (2000, 0.40)];
        let before = lane.points().to_vec();

        let linear = [
            (0, 0.20),
            (500, 0.50),
            (999, 0.7994),
            (1000, 0.80),
            (1500, 0.60),
            (1999, 0.4004),
            (2000, 0.40),
            (2500, 0.40),
        ];
        for &(sample, value) in &linear {
            let got = lane.curve(sample);
            assert!(
                (got - value).abs() <= 1e-4,
                "linear curve({sample}) = {got}, expected {value}"
            );
        }

        lane.set_interpolation(AutomationInterpolation::Discrete);
        assert_eq!(lane.points(), before.as_slice());
        let discrete = [
            (0, 0.20),
            (500, 0.20),
            (999, 0.20),
            (1000, 0.80),
            (1500, 0.80),
            (1999, 0.80),
            (2000, 0.40),
            (2500, 0.40),
        ];
        for &(sample, value) in &discrete {
            let got = lane.curve(sample);
            assert!(
                (got - value).abs() <= 1e-4,
                "discrete curve({sample}) = {got}, expected {value}"
            );
        }
        assert_eq!(lane.points().len(), 3);
        assert_eq!(
            lane.points()
                .iter()
                .map(|point| point.0)
                .collect::<Vec<_>>(),
            vec![0, 1000, 2000]
        );
    }

    #[test]
    fn acceptance_05_single_point_hold() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
        lane.points = vec![(1000, 0.80)];
        for interpolation in [
            AutomationInterpolation::Linear,
            AutomationInterpolation::Discrete,
        ] {
            lane.set_interpolation(interpolation);
            for sample in [0_u64, 500, 1000, 2000] {
                let got = lane.curve(sample);
                assert!(
                    (got - 0.80).abs() <= 1e-4,
                    "{interpolation:?} curve({sample}) = {got}"
                );
            }
            assert_eq!(lane.points().len(), 1);
            assert_eq!(lane.points()[0].0, 1000);
            approx(lane.points()[0].1, 0.80);
        }
    }

    #[test]
    fn acceptance_06_manual_roll_keeps_hand_and_points() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
        lane.points = vec![(0, 0.20), (1000, 0.80), (2000, 0.40)];
        lane.set_hand(0.33);
        lane.set_rolling(true);
        for sample in [0_u64, 1, 500, 1000] {
            lane.set_playhead(sample);
            approx(lane.heard(), 0.33);
        }
        approx(lane.hand(), 0.33);
        approx(lane.curve(500), 0.50);
        assert_eq!(lane.points(), &[(0, 0.20), (1000, 0.80), (2000, 0.40)]);
    }

    #[test]
    fn acceptance_07_play_roll_ignores_gesture() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.25);
        lane.points = vec![(0, 0.20), (48_000, 0.80)];
        lane.set_mode(AutomationMode::Play);
        lane.set_rolling(true);
        lane.set_playhead(6144);
        approx(lane.heard(), 0.2768);
        let points = lane.points().to_vec();
        lane.set_hand(0.11);
        approx(lane.heard(), 0.2768);
        approx(lane.hand(), 0.25);
        assert_eq!(lane.points(), points.as_slice());
        lane.set_rolling(false);
        assert_eq!(lane.mode(), AutomationMode::Play);
        assert_eq!(lane.points(), points.as_slice());
    }

    #[test]
    fn acceptance_08_play_stopped_follow_playhead() {
        let mut lane = AutomationLane::new(0.0, 1.0, 1.0);
        lane.points = vec![(0, 0.20), (1000, 0.80), (2000, 0.40)];
        lane.set_mode(AutomationMode::Play);
        assert!(!lane.rolling());
        assert!((lane.heard() - 1.0).abs() <= 1e-4);
        lane.set_playhead(1500);
        approx(lane.heard(), 0.60);
        lane.set_playhead(0);
        approx(lane.heard(), lane.curve(0));
        approx(lane.heard(), 0.20);
    }

    #[test]
    fn acceptance_09_entering_play_does_not_replace_heard() {
        let mut lane = AutomationLane::new(0.0, 1.0, 1.0);
        lane.points = vec![(0, 0.20), (1000, 0.80)];
        assert!((lane.curve(0) - 0.20).abs() <= 1e-4);
        lane.set_mode(AutomationMode::Play);
        assert!(!lane.rolling());
        assert_eq!(lane.playhead(), 0);
        approx(lane.hand(), 1.0);
        approx(lane.heard(), 1.0);
    }

    #[test]
    fn acceptance_10_write_roll_hears_preset_hand_then_touch() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
        lane.points = vec![(0, 0.20), (48_000, 0.80)];
        let points = lane.points().to_vec();
        lane.set_mode(AutomationMode::Write);
        lane.set_hand(0.30);
        assert_eq!(lane.points(), points.as_slice());
        lane.set_rolling(true);
        lane.set_playhead(8192);
        approx(lane.heard(), 0.30);
        approx(lane.hand(), 0.30);
        assert_eq!(lane.points(), points.as_slice());
        lane.set_rolling(false);
        assert_eq!(lane.mode(), AutomationMode::Touch);
        assert!(!lane.rolling());
        approx(lane.heard(), 0.3024);
        approx(lane.heard(), lane.curve(8192));
        assert_eq!(lane.points(), points.as_slice());
    }

    #[test]
    fn acceptance_11_rolling_gesture_punches_triplet() {
        for (mode, hold) in [
            (AutomationMode::Write, false),
            (AutomationMode::Touch, true),
            (AutomationMode::Latch, false),
        ] {
            let mut lane = AutomationLane::new(0.0, 1.0, 0.20);
            lane.points = vec![(0, 0.20), (48_000, 0.80)];
            lane.set_mode(mode);
            if hold {
                lane.touch(true);
            }
            lane.set_rolling(true);
            lane.set_playhead(3072);
            lane.set_hand(0.55);
            assert_eq!(lane.points().len(), 5, "{mode:?}");
            let checks = [
                (0, 0.20),
                (3008, 0.2376),
                (3072, 0.55),
                (3136, 0.80),
                (48_000, 0.80),
            ];
            for &(sample, value) in &checks {
                let got = point(&lane, sample);
                assert!(
                    (got - value).abs() <= 1e-4,
                    "{mode:?} point {sample} = {got}, expected {value}"
                );
            }
        }
    }

    #[test]
    fn acceptance_12_isolated_triplets() {
        for (mode, hold) in [
            (AutomationMode::Write, false),
            (AutomationMode::Touch, true),
            (AutomationMode::Latch, false),
        ] {
            let mut lane = AutomationLane::new(0.0, 1.0, 0.20);
            lane.points = vec![(0, 0.20), (48_000, 0.20)];
            lane.set_mode(mode);
            if hold {
                lane.touch(true);
            }
            lane.set_rolling(true);
            for sample in [2048_u64, 4096, 6144] {
                lane.set_playhead(sample);
                lane.set_hand(0.50);
            }
            assert_eq!(lane.points().len(), 11, "{mode:?}");
            let anchors = [
                (0, 0.20),
                (1984, 0.20),
                (2048, 0.50),
                (2112, 0.20),
                (4032, 0.20),
                (4096, 0.50),
                (4160, 0.20),
                (6080, 0.20),
                (6144, 0.50),
                (6208, 0.20),
                (48_000, 0.20),
            ];
            for &(sample, value) in &anchors {
                let got = point(&lane, sample);
                assert!(
                    (got - value).abs() <= 1e-4,
                    "{mode:?} point {sample} = {got}, expected {value}"
                );
            }
            for sample in [100_u64, 3000, 5000, 7000] {
                let got = lane.curve(sample);
                assert!(
                    (got - 0.20).abs() <= 1e-4,
                    "{mode:?} curve({sample}) = {got}, expected 0.20"
                );
            }
        }
    }

    #[test]
    fn acceptance_13_touch_up_ignores_gesture() {
        let mut lane = AutomationLane::new(0.0, 1.0, 1.0);
        lane.points = vec![(0, 0.20), (1000, 0.80), (2000, 0.40)];
        lane.set_mode(AutomationMode::Touch);
        assert!(!lane.touching());
        lane.set_rolling(true);
        lane.set_playhead(500);
        approx(lane.heard(), 0.50);
        let points = lane.points().to_vec();
        lane.set_hand(0.70);
        approx(lane.heard(), 0.50);
        approx(lane.curve(500), 0.50);
        approx(lane.hand(), 1.0);
        assert_eq!(lane.points(), points.as_slice());
    }

    #[test]
    fn acceptance_14_touch_held_hears_hand_until_release() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.20);
        lane.points = vec![(0, 0.20), (48_000, 0.20)];
        lane.set_mode(AutomationMode::Touch);
        lane.touch(true);
        lane.set_rolling(true);
        lane.set_playhead(2048);
        lane.set_hand(0.60);
        approx(lane.heard(), 0.60);
        lane.set_playhead(3000);
        approx(lane.heard(), 0.60);
        lane.touch(false);
        assert!(lane.rolling());
        approx(lane.curve(3000), 0.20);
        approx(lane.heard(), 0.20);
        lane.set_rolling(false);
        assert_eq!(lane.mode(), AutomationMode::Touch);
        assert!(!lane.rolling());
    }

    #[test]
    fn acceptance_15_latch_without_gesture() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
        lane.points = vec![(0, 0.20), (48_000, 0.80)];
        lane.set_mode(AutomationMode::Latch);
        lane.set_rolling(true);
        lane.set_playhead(3072);
        approx(lane.heard(), 0.2384);
        assert_eq!(lane.points().len(), 2);
        lane.set_rolling(false);
        assert_eq!(lane.mode(), AutomationMode::Latch);
        assert_eq!(lane.points().len(), 2);
        assert!(!lane.rolling());
    }

    #[test]
    fn acceptance_16_latch_holds_hand_until_stop() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.20);
        lane.points = vec![(0, 0.20), (48_000, 0.20)];
        lane.set_mode(AutomationMode::Latch);
        lane.set_rolling(true);
        lane.set_playhead(2048);
        lane.set_hand(0.50);
        approx(lane.heard(), 0.50);
        lane.set_playhead(3000);
        approx(lane.heard(), 0.50);
        lane.set_playhead(6144);
        approx(lane.heard(), 0.50);
        lane.set_rolling(false);
        assert_eq!(lane.mode(), AutomationMode::Latch);
        approx(lane.curve(6144), 0.20);
        approx(lane.heard(), 0.20);
        lane.set_rolling(true);
        lane.set_playhead(3000);
        approx(lane.heard(), 0.20);
        approx(lane.curve(3000), 0.20);
    }

    #[test]
    fn acceptance_17_stopped_touch_replaces_playhead_point() {
        let mut lane = AutomationLane::new(0.0, 1.0, 0.20);
        lane.points = vec![(0, 0.20)];
        lane.set_mode(AutomationMode::Touch);
        lane.touch(true);
        assert!(!lane.rolling());
        assert_eq!(lane.playhead(), 0);
        lane.set_hand(0.30);
        approx(lane.heard(), 0.30);
        approx(lane.hand(), 0.30);
        assert_eq!(lane.points().len(), 1);
        assert_eq!(lane.points()[0].0, 0);
        approx(lane.points()[0].1, 0.30);
        assert!(lane.points().iter().all(|point| point.0 != 64));
    }
}
