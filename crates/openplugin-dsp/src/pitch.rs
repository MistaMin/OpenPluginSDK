//! MIDI note number and Hertz.
//!
//! `pitch_calc` (MIT OR Apache-2.0) exposes the same equal-temperament pair,
//! `hz_from_step` and `step_from_hz`, with A4 = 69 at 440 Hz. These two
//! functions are that pair without the letter, mel, and scale graphs.

const A4_NOTE: f32 = 69.0;
const A4_HZ: f32 = 440.0;

/// Hertz for a MIDI note. `69` is A4 at 440 Hz. Non-finite notes become `0`.
pub fn hz_from_midi(note: f32) -> f32 {
    if !note.is_finite() {
        return 0.0;
    }
    A4_HZ * 2f32.powf((note - A4_NOTE) / 12.0)
}

/// MIDI note for a frequency. `None` when `hz` is not a positive finite number.
pub fn midi_from_hz(hz: f32) -> Option<f32> {
    if !hz.is_finite() || hz <= 0.0 {
        return None;
    }
    Some(A4_NOTE + 12.0 * hz.log2() - 12.0 * A4_HZ.log2())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_is_440_and_one_octave_doubles() {
        assert!((hz_from_midi(69.0) - 440.0).abs() < 1e-3);
        assert!((hz_from_midi(81.0) - 880.0).abs() < 1e-2);
        let back = midi_from_hz(440.0).unwrap();
        assert!((back - 69.0).abs() < 1e-3);
        assert!(midi_from_hz(0.0).is_none());
        assert_eq!(hz_from_midi(f32::NAN), 0.0);
    }
}
