//! Tempo, milliseconds, and sample counts.
//!
//! `time_calc` (MIT OR Apache-2.0) is this conversion set, plus bars and a
//! random helper. The functions here are the ones a plugin transport needs.
//! A non-positive tempo or sample rate returns `None` instead of dividing.

/// Samples that fit in `ms` at `sample_rate`.
pub fn samples_from_ms(sample_rate: f32, ms: f32) -> Option<f32> {
    rate(sample_rate)?;
    finite(ms)?;
    Some(ms * sample_rate / 1000.0)
}

/// Milliseconds occupied by `samples` at `sample_rate`.
pub fn ms_from_samples(sample_rate: f32, samples: f32) -> Option<f32> {
    rate(sample_rate)?;
    finite(samples)?;
    Some(samples * 1000.0 / sample_rate)
}

/// One beat, in milliseconds. `bpm` is quarter notes per minute.
pub fn ms_per_beat(bpm: f32) -> Option<f32> {
    tempo(bpm)?;
    Some(60_000.0 / bpm)
}

/// Samples in one quarter-note beat.
pub fn samples_per_beat(sample_rate: f32, bpm: f32) -> Option<f32> {
    let ms = ms_per_beat(bpm)?;
    samples_from_ms(sample_rate, ms)
}

/// Samples spanned by `ticks` at `ppqn` ticks per quarter note.
pub fn samples_from_ticks(sample_rate: f32, bpm: f32, ppqn: f32, ticks: f32) -> Option<f32> {
    let beat = samples_per_beat(sample_rate, bpm)?;
    if !ppqn.is_finite() || ppqn <= 0.0 || !ticks.is_finite() {
        return None;
    }
    Some(ticks * beat / ppqn)
}

fn rate(sample_rate: f32) -> Option<()> {
    if sample_rate.is_finite() && sample_rate > 0.0 {
        Some(())
    } else {
        None
    }
}

fn tempo(bpm: f32) -> Option<()> {
    if bpm.is_finite() && bpm > 0.0 {
        Some(())
    } else {
        None
    }
}

fn finite(value: f32) -> Option<()> {
    if value.is_finite() {
        Some(())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_beat_at_120_bpm_is_half_a_second() {
        assert!((ms_per_beat(120.0).unwrap() - 500.0).abs() < 1e-3);
        let samples = samples_per_beat(48_000.0, 120.0).unwrap();
        assert!((samples - 24_000.0).abs() < 1e-2);
        assert!(samples_per_beat(48_000.0, 0.0).is_none());
        let ticks = samples_from_ticks(48_000.0, 120.0, 24.0, 24.0).unwrap();
        assert!((ticks - 24_000.0).abs() < 1e-2);
        assert!((samples_from_ms(48_000.0, 1000.0).unwrap() - 48_000.0).abs() < 1e-2);
        assert!((ms_from_samples(48_000.0, 48_000.0).unwrap() - 1000.0).abs() < 1e-2);
    }
}
