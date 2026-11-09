//! Sample-accurate parameter ramp.
//!
//! The VST 3 SDK (MIT, Steinberg 2026) walks a queue of (offset, value)
//! points across a block. This ramp is that walk for one parameter: values
//! between two points are linear, and the audio thread does not allocate.
//! `vstgui` in that tree is BSD-3-Clause and is not used here.

const POINTS: usize = 32;

/// One parameter across one block.
#[derive(Clone, Debug)]
pub struct SampleRamp {
    value: f32,
    origin_sample: u32,
    origin_value: f32,
    points: [(u32, f32); POINTS],
    count: usize,
    cursor: usize,
    sample: u32,
}

impl SampleRamp {
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            origin_sample: 0,
            origin_value: value,
            points: [(0, 0.0); POINTS],
            count: 0,
            cursor: 0,
            sample: 0,
        }
    }

    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Replace the value and drop any queued points.
    pub fn set(&mut self, value: f32) {
        if value.is_finite() {
            self.value = value;
            self.origin_value = value;
        }
        self.count = 0;
        self.cursor = 0;
    }

    /// Start a block. Queued points from the previous block are dropped.
    /// The value left by the last sample carries in.
    pub fn begin(&mut self, _frames: u32) {
        self.count = 0;
        self.cursor = 0;
        self.sample = 0;
        self.origin_sample = 0;
        self.origin_value = self.value;
    }

    /// Queue a point at `offset` samples into the block. Points must be
    /// appended in non-decreasing offset order. Returns false when the queue
    /// is full or the value is not finite.
    pub fn push(&mut self, offset: u32, value: f32) -> bool {
        if !value.is_finite() || self.count >= POINTS {
            return false;
        }
        if self.count > 0 && offset < self.points[self.count - 1].0 {
            return false;
        }
        self.points[self.count] = (offset, value);
        self.count += 1;
        true
    }

    /// Value at the current sample, then advance one sample.
    pub fn next(&mut self) -> f32 {
        self.pull_due();
        let y = if self.cursor < self.count {
            let (at, target) = self.points[self.cursor];
            let span = at.saturating_sub(self.origin_sample);
            if span == 0 {
                target
            } else {
                let t = (self.sample - self.origin_sample) as f32 / span as f32;
                self.origin_value + (target - self.origin_value) * t
            }
        } else {
            self.value
        };
        self.sample = self.sample.saturating_add(1);
        y
    }

    fn pull_due(&mut self) {
        while self.cursor < self.count && self.sample >= self.points[self.cursor].0 {
            let (at, target) = self.points[self.cursor];
            self.value = target;
            self.origin_sample = at;
            self.origin_value = target;
            self.cursor += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_hits_each_point_and_holds_the_last_value() {
        let mut ramp = SampleRamp::new(0.0);
        ramp.begin(8);
        assert!(ramp.push(4, 1.0));
        let mut got = [0.0; 8];
        for sample in &mut got {
            *sample = ramp.next();
        }
        assert!(got[0].abs() < 1e-5);
        assert!((got[2] - 0.5).abs() < 1e-5);
        assert!((got[4] - 1.0).abs() < 1e-5);
        assert!((got[7] - 1.0).abs() < 1e-5);
        assert!((ramp.value() - 1.0).abs() < 1e-5);
    }
}
