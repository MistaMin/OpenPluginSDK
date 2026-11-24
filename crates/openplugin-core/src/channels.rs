//! Channel slices over buffers a host owns, for [`crate::Plugin::process`].
//!
//! `process` takes `&mut [&mut [f32]]`. A host that keeps its audio as
//! `Vec<Vec<f32>>` borrows it through [`ChannelsMut`] (and a sidechain through
//! [`Channels`]) without allocating:
//!
//! ```
//! # use openplugin_core::ChannelsMut;
//! let mut io = vec![vec![0.0_f32; 64]; 2];
//! let mut channels = ChannelsMut::new(&mut io);
//! let slices: &mut [&mut [f32]] = &mut channels;
//! assert_eq!(slices.len(), 2);
//! ```

use std::ops::{Deref, DerefMut};

use crate::MAX_AUDIO_CHANNELS;

/// One mutable slice per buffer, at most [`MAX_AUDIO_CHANNELS`] of them (the
/// rest are left out). Dereferences to `[&mut [f32]]`.
pub struct ChannelsMut<'a> {
    slices: [&'a mut [f32]; MAX_AUDIO_CHANNELS],
    len: usize,
}

impl<'a> ChannelsMut<'a> {
    /// Borrow each buffer as a channel; past [`MAX_AUDIO_CHANNELS`] buffers
    /// the rest are left out. Does not allocate (the slices live inline).
    pub fn new<B: AsMut<[f32]>>(buffers: &'a mut [B]) -> Self {
        let mut slices: [&'a mut [f32]; MAX_AUDIO_CHANNELS] = std::array::from_fn(|_| Default::default());
        let len = buffers.len().min(MAX_AUDIO_CHANNELS);
        for (slice, buffer) in slices.iter_mut().zip(buffers) {
            *slice = buffer.as_mut();
        }
        Self { slices, len }
    }
}

impl<'a> Deref for ChannelsMut<'a> {
    type Target = [&'a mut [f32]];
    fn deref(&self) -> &Self::Target {
        &self.slices[..self.len]
    }
}

impl DerefMut for ChannelsMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.slices[..self.len]
    }
}

/// One shared slice per buffer, at most [`MAX_AUDIO_CHANNELS`] of them.
/// Dereferences to `[&[f32]]`.
pub struct Channels<'a> {
    slices: [&'a [f32]; MAX_AUDIO_CHANNELS],
    len: usize,
}

impl<'a> Channels<'a> {
    /// Borrow each buffer as a channel; past [`MAX_AUDIO_CHANNELS`] buffers
    /// the rest are left out. Does not allocate.
    ///
    /// ```
    /// use openplugin_core::Channels;
    /// let sidechain = vec![vec![0.5_f32; 32]; 2];
    /// let channels = Channels::new(&sidechain);
    /// let slices: &[&[f32]] = &channels;
    /// assert_eq!(slices[1][0], 0.5);
    /// ```
    pub fn new<B: AsRef<[f32]>>(buffers: &'a [B]) -> Self {
        let mut slices: [&'a [f32]; MAX_AUDIO_CHANNELS] = [&[]; MAX_AUDIO_CHANNELS];
        let len = buffers.len().min(MAX_AUDIO_CHANNELS);
        for (slice, buffer) in slices.iter_mut().zip(buffers) {
            *slice = buffer.as_ref();
        }
        Self { slices, len }
    }
}

impl<'a> Deref for Channels<'a> {
    type Target = [&'a [f32]];
    fn deref(&self) -> &Self::Target {
        &self.slices[..self.len]
    }
}
