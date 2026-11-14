//! The format-neutral editor contract.
//!
//! An editor is the plugin's own window content. The SDK hosts it in every
//! format: CLAP (`clap.gui`), LV2 (`ui:CocoaUI`), and the VST3, Audio Unit and
//! AAX shells (`op_editor_*`). The plugin writes one [`Editor`] and one
//! [`EditorFactory`]; the format bridges do the rest.
//!
//! The rules are the same in every format:
//!
//! * Every [`Editor`] call is on the main thread.
//! * The editor never writes to the engine. It reports a change through
//!   [`EditorHost`] as a gesture ([`Gesture`]) in plain units, and the host
//!   brings the value back to the engine as an ordinary parameter change.
//! * Values that change elsewhere (automation, a preset the host loads) reach
//!   the editor through [`Editor::param_changed`].
//! * An editor exists before it has an engine, and the engine it shows can be
//!   replaced while it is open (a shell recreates its engine when the host
//!   reconfigures it). [`Editor::bind`] hands it the [`EditorState`] that the
//!   engine published through [`crate::Plugin::editor_state`], or `None`.
//!
//! Size: an editor is fixed-size unless it says otherwise. [`Editor::size_range`]
//! gives the sizes it can take ([`SizeRange`]) and [`Editor::resize`] is how a
//! host moves it to one of them; [`Editor::size`] is always the current size.
//! An editor asks its host for a size with [`EditorHost::request_resize`].
//!
//! Pixels are 32-bit words `0x00RRGGBB`, rows top to bottom, which is what
//! the macOS view in `openplugin-dri` blits.

use std::any::Any;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// What an engine shares with its editor: meters, an analyser ring, display
/// choices that survive a reopen. The plugin defines the type and the editor
/// downcasts it (`state.downcast::<MyShared>()`).
pub type EditorState = Arc<dyn Any + Send + Sync>;

/// One editor gesture on one parameter, by descriptor index, in plain units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    Begin(u32),
    Perform(u32, f32),
    End(u32),
}

/// Where an editor sends its gestures. Created by the format bridge; cheap to
/// clone, main thread only.
#[derive(Clone)]
pub struct EditorHost {
    sink: Rc<dyn Fn(Gesture)>,
    resize: Option<Rc<dyn Fn(u32, u32) -> bool>>,
}

impl EditorHost {
    pub fn new(sink: impl Fn(Gesture) + 'static) -> Self {
        Self { sink: Rc::new(sink), resize: None }
    }

    /// The same host, with a way to ask the format's host for a new editor
    /// size. The handler returns whether the host agreed; the host then
    /// resizes the editor through its own calls (`set_size`, `ui:resize`).
    pub fn with_resize(mut self, request: impl Fn(u32, u32) -> bool + 'static) -> Self {
        self.resize = Some(Rc::new(request));
        self
    }

    /// Ask the host to give the editor `width` x `height` points. The size is
    /// brought into the editor's [`SizeRange`] first. Returns whether the host
    /// agreed; a `false` leaves the editor as it is. The editor is resized
    /// later, through [`Editor::resize`], and not inside this call.
    pub fn request_resize(&self, range: &SizeRange, width: u32, height: u32) -> bool {
        let (w, h) = range.clamp(width, height);
        match &self.resize {
            Some(request) => request(w, h),
            None => false,
        }
    }

    /// A host that drops every gesture (previews, tests, a standalone window
    /// that applies values itself).
    pub fn detached() -> Self {
        Self::new(|_| {})
    }

    pub fn send(&self, gesture: Gesture) {
        (self.sink)(gesture);
    }

    /// The user grabbed a control.
    pub fn begin(&self, index: usize) {
        self.send(Gesture::Begin(index as u32));
    }

    /// A new plain value while the control is held.
    pub fn perform(&self, index: usize, plain: f32) {
        if plain.is_finite() {
            self.send(Gesture::Perform(index as u32, plain));
        }
    }

    /// The user let go.
    pub fn end(&self, index: usize) {
        self.send(Gesture::End(index as u32));
    }

    /// A complete change in one call: begin, perform, end. For clicks, steps
    /// and preset loads, where there is no drag to hold the gesture open.
    pub fn set(&self, index: usize, plain: f32) {
        self.begin(index);
        self.perform(index, plain);
        self.end(index);
    }
}

/// A rendered editor frame. `pixels.len() == width * height`.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub pixels: &'a [u32],
    pub width: usize,
    pub height: usize,
}

/// The sizes an editor can take, in points. [`SizeRange::fixed`] is the
/// default for every editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeRange {
    pub min: (u32, u32),
    pub max: (u32, u32),
    /// Width and height of the shape to keep (3, 2 for 3:2), or `None` for any shape.
    pub aspect: Option<(u32, u32)>,
}

impl SizeRange {
    /// One size only.
    pub const fn fixed(width: u32, height: u32) -> Self {
        Self { min: (width, height), max: (width, height), aspect: None }
    }

    /// Any size from `min` to `max`, in any shape.
    pub const fn between(min: (u32, u32), max: (u32, u32)) -> Self {
        Self { min, max, aspect: None }
    }

    /// The same range, keeping the shape `width`:`height`.
    pub const fn with_aspect(mut self, width: u32, height: u32) -> Self {
        self.aspect = Some((width, height));
        self
    }

    /// Bounds in order and at least 1 x 1, whatever was written.
    fn bounds(&self) -> ((u32, u32), (u32, u32)) {
        let min = (self.min.0.max(1), self.min.1.max(1));
        (min, (self.max.0.max(min.0), self.max.1.max(min.1)))
    }

    /// The editor takes one size only.
    pub fn is_fixed(&self) -> bool {
        let (min, max) = self.bounds();
        min == max
    }

    pub fn can_resize_horizontally(&self) -> bool {
        let (min, max) = self.bounds();
        min.0 < max.0
    }

    pub fn can_resize_vertically(&self) -> bool {
        let (min, max) = self.bounds();
        min.1 < max.1
    }

    /// The shape to keep, if both numbers are set.
    pub fn aspect_ratio(&self) -> Option<(u32, u32)> {
        self.aspect.filter(|a| a.0 > 0 && a.1 > 0)
    }

    /// Whether `width` x `height` is inside the bounds (the shape is a hint).
    pub fn contains(&self, width: u32, height: u32) -> bool {
        let (min, max) = self.bounds();
        (min.0..=max.0).contains(&width) && (min.1..=max.1).contains(&height)
    }

    /// The size nearest to a request that the editor can take. Without an
    /// aspect each side is held to its bounds. With one, the result has that
    /// shape (to a point) and the area nearest to the request, inside both
    /// bounds. Clamping a result again changes nothing.
    pub fn clamp(&self, width: u32, height: u32) -> (u32, u32) {
        let (min, max) = self.bounds();
        let Some((aw, ah)) = self.aspect_ratio() else {
            return (width.clamp(min.0, max.0), height.clamp(min.1, max.1));
        };
        let (aw, ah) = (f64::from(aw), f64::from(ah));
        // The size is `s * (aw, ah)`; find `s` for the requested area, inside both bounds.
        let wanted = (f64::from(width.max(1)) * f64::from(height.max(1)) / (aw * ah)).sqrt();
        let low = (f64::from(min.0) / aw).max(f64::from(min.1) / ah);
        let high = (f64::from(max.0) / aw).min(f64::from(max.1) / ah).max(low);
        let s = wanted.clamp(low, high);
        // Width decides; height follows from it, so a result is its own fixed point.
        let w = ((s * aw).round() as u32).clamp(min.0, max.0);
        let h = ((f64::from(w) * ah / aw).round() as u32).clamp(min.1, max.1);
        (w, h)
    }
}

/// The plugin's editor. Main thread only; it is never sent to another thread.
pub trait Editor {
    /// The current size in points (logical pixels). A fixed editor never
    /// changes it; a resizable one changes it in [`Self::resize`].
    fn size(&self) -> (u32, u32);

    /// The sizes this editor can take. The default is the current size alone,
    /// so an editor that does not override this and [`Self::resize`] stays
    /// fixed in every format.
    fn size_range(&self) -> SizeRange {
        let (w, h) = self.size();
        SizeRange::fixed(w, h)
    }

    /// The host gives the editor a new size in points. Called only with a size
    /// inside [`Self::size_range`]. Afterwards [`Self::size`] is that size and
    /// [`Self::render`] draws at it. The default ignores it.
    fn resize(&mut self, _width: u32, _height: u32) {}

    /// Show the engine whose state this is, or no engine. Called after
    /// creation when the engine is known, and again whenever it changes.
    fn bind(&mut self, _state: Option<EditorState>) {}

    /// A parameter value from the host, plain. Only called when it changed.
    fn param_changed(&mut self, index: usize, plain: f32);

    /// Animation step, about 30 times a second. `dt` is in seconds.
    fn tick(&mut self, _dt: f32) {}

    /// Draw the editor at `scale` device pixels per point.
    fn render(&mut self, scale: f32) -> Frame<'_>;

    /// Mouse input in points, origin at the top left.
    fn mouse_down(&mut self, _x: f32, _y: f32, _clicks: u32) {}
    fn mouse_move(&mut self, _x: f32, _y: f32) {}
    fn mouse_up(&mut self, _x: f32, _y: f32) {}
    /// Wheel or trackpad scroll; positive `dy` is away from the user.
    fn wheel(&mut self, _x: f32, _y: f32, _dy: f32) {}
}

/// Makes the plugin's editor. Passed to `export_clap!`, `export_lv2!` and
/// `export_shell!` as `editor = path::to::fn`. `None` means no editor could
/// be made; the host then shows its generic controls.
pub type EditorFactory = fn(EditorHost) -> Option<Box<dyn Editor>>;

/// Remembers the last value given to an editor per parameter, so a bridge
/// that polls values calls [`Editor::param_changed`] only on a change.
pub struct ValueMirror {
    seen: Vec<u32>,
    primed: bool,
}

impl ValueMirror {
    pub fn new(count: usize) -> Self {
        Self { seen: vec![0; count], primed: false }
    }

    /// Forget every value, so the next [`Self::update`] reports all of them.
    pub fn invalidate(&mut self) {
        self.primed = false;
    }

    /// Report `value(index)` for every index whose bits changed.
    pub fn update(&mut self, editor: &mut dyn Editor, value: impl Fn(usize) -> Option<f32>) {
        let all = !self.primed;
        for (index, seen) in self.seen.iter_mut().enumerate() {
            let Some(v) = value(index) else { continue };
            if !v.is_finite() {
                continue;
            }
            if all || v.to_bits() != *seen {
                *seen = v.to_bits();
                editor.param_changed(index, v);
            }
        }
        self.primed = true;
    }
}

/// Fixed-capacity gesture queue with one producer and one consumer that may
/// each be on any thread. Both ends take `&self`, so the queue can live in an
/// `Arc` shared by the editor (producer, main thread) and the audio thread or
/// a parameter flush (consumer). Push and pop are wait-free and do not
/// allocate. A full queue rejects the gesture.
pub struct GestureQueue {
    words: Box<[AtomicU32]>,
    values: Box<[AtomicU32]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

const KIND_BEGIN: u32 = 1;
const KIND_PERFORM: u32 = 2;
const KIND_END: u32 = 3;
const INDEX_MASK: u32 = 0x00ff_ffff;

impl GestureQueue {
    /// Room for `capacity` gestures (at least one).
    pub fn new(capacity: usize) -> Self {
        let slots = capacity.max(1) + 1;
        Self {
            words: (0..slots).map(|_| AtomicU32::new(0)).collect(),
            values: (0..slots).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Producer side. False when full or when the index does not fit 24 bits.
    pub fn push(&self, gesture: Gesture) -> bool {
        let (kind, index, value) = match gesture {
            Gesture::Begin(i) => (KIND_BEGIN, i, 0.0),
            Gesture::Perform(i, v) => (KIND_PERFORM, i, v),
            Gesture::End(i) => (KIND_END, i, 0.0),
        };
        if index > INDEX_MASK {
            return false;
        }
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % self.words.len();
        if next == self.tail.load(Ordering::Acquire) {
            return false;
        }
        self.words[head].store((kind << 24) | index, Ordering::Relaxed);
        self.values[head].store(value.to_bits(), Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
        true
    }

    /// Consumer side.
    pub fn pop(&self) -> Option<Gesture> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let word = self.words[tail].load(Ordering::Relaxed);
        let value = f32::from_bits(self.values[tail].load(Ordering::Relaxed));
        self.tail.store((tail + 1) % self.words.len(), Ordering::Release);
        let index = word & INDEX_MASK;
        Some(match word >> 24 {
            KIND_BEGIN => Gesture::Begin(index),
            KIND_PERFORM => Gesture::Perform(index, value),
            _ => Gesture::End(index),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Acquire) == self.head.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_keeps_order_and_rejects_when_full() {
        let q = GestureQueue::new(3);
        assert!(q.push(Gesture::Begin(7)));
        assert!(q.push(Gesture::Perform(7, -2.5)));
        assert!(q.push(Gesture::End(7)));
        assert!(!q.push(Gesture::Begin(1)), "full");
        assert_eq!(q.pop(), Some(Gesture::Begin(7)));
        assert_eq!(q.pop(), Some(Gesture::Perform(7, -2.5)));
        assert_eq!(q.pop(), Some(Gesture::End(7)));
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn queue_works_across_threads() {
        let q = Arc::new(GestureQueue::new(64));
        let producer = {
            let q = q.clone();
            std::thread::spawn(move || {
                for i in 0..10_000u32 {
                    while !q.push(Gesture::Perform(i % 100, i as f32)) {
                        std::thread::yield_now();
                    }
                }
            })
        };
        let mut next = 0u32;
        while next < 10_000 {
            if let Some(Gesture::Perform(i, v)) = q.pop() {
                assert_eq!((i, v), (next % 100, next as f32));
                next += 1;
            }
        }
        producer.join().unwrap();
    }

    #[test]
    fn size_range_clamps_each_side_without_an_aspect() {
        let r = SizeRange::between((100, 50), (400, 200));
        assert_eq!(r.clamp(10, 10), (100, 50));
        assert_eq!(r.clamp(1000, 1000), (400, 200));
        assert_eq!(r.clamp(250, 60), (250, 60));
        assert_eq!(r.clamp(0, 0), (100, 50));
        assert!(r.contains(100, 200) && !r.contains(99, 100) && !r.contains(100, 201));
        assert!(r.can_resize_horizontally() && r.can_resize_vertically() && !r.is_fixed());
    }

    #[test]
    fn fixed_range_always_gives_its_size() {
        let r = SizeRange::fixed(240, 160);
        assert!(r.is_fixed() && !r.can_resize_horizontally() && !r.can_resize_vertically());
        for (w, h) in [(0, 0), (240, 160), (1, 9999), (u32::MAX, u32::MAX)] {
            assert_eq!(r.clamp(w, h), (240, 160));
        }
        // Written upside down, the bounds are put in order.
        let odd = SizeRange { min: (300, 300), max: (10, 10), aspect: Some((0, 5)) };
        assert_eq!(odd.clamp(5, 5), (300, 300));
        assert!(odd.is_fixed());
    }

    #[test]
    fn aspect_keeps_the_shape_inside_the_bounds() {
        let r = SizeRange::between((180, 120), (480, 320)).with_aspect(3, 2);
        assert_eq!(r.clamp(240, 160), (240, 160));
        for (w, h) in [(300, 160), (100, 100), (900, 100), (1, 1), (481, 321), (333, 777), (u32::MAX, 1)] {
            let (cw, ch) = r.clamp(w, h);
            assert!(r.contains(cw, ch), "{w}x{h} gave {cw}x{ch}");
            assert!((i64::from(cw) * 2 - i64::from(ch) * 3).abs() <= 2, "{w}x{h} gave {cw}x{ch}");
            assert_eq!(r.clamp(cw, ch), (cw, ch), "a result is a fixed point");
        }
        assert_eq!(r.clamp(2000, 2000), (480, 320));
        assert_eq!(r.clamp(1, 1), (180, 120));
        for (aw, ah) in [(3, 2), (16, 9), (1, 1), (7, 5)] {
            let r = SizeRange::between((151, 97), (611, 433)).with_aspect(aw, ah);
            for w in (1..800).step_by(13) {
                for h in (1..600).step_by(11) {
                    let c = r.clamp(w, h);
                    assert!(r.contains(c.0, c.1), "{aw}:{ah} {w}x{h} gave {c:?}");
                    assert_eq!(r.clamp(c.0, c.1), c, "{aw}:{ah} {w}x{h} gave {c:?}");
                }
            }
        }
    }

    #[test]
    fn aspect_with_bounds_of_another_shape_stays_inside_them() {
        // 1:1 inside a wide box: the height limit decides.
        let r = SizeRange::between((100, 100), (400, 200)).with_aspect(1, 1);
        assert_eq!(r.clamp(900, 900), (200, 200));
        assert_eq!(r.clamp(5, 5), (100, 100));
    }

    #[test]
    fn default_editor_is_fixed_and_ignores_resize() {
        let mut probe = Probe(Vec::new());
        assert_eq!(probe.size_range(), SizeRange::fixed(1, 1));
        probe.resize(50, 50);
        assert_eq!(probe.size(), (1, 1));
    }

    #[test]
    fn request_resize_needs_a_handler_and_clamps() {
        let range = SizeRange::between((10, 10), (100, 100));
        assert!(!EditorHost::detached().request_resize(&range, 50, 50));
        let seen = Rc::new(std::cell::Cell::new((0, 0)));
        let sink = seen.clone();
        let host = EditorHost::detached().with_resize(move |w, h| {
            sink.set((w, h));
            true
        });
        assert!(host.request_resize(&range, 500, 5));
        assert_eq!(seen.get(), (100, 10));
    }

    struct Probe(Vec<(usize, f32)>);
    impl Editor for Probe {
        fn size(&self) -> (u32, u32) {
            (1, 1)
        }
        fn param_changed(&mut self, index: usize, plain: f32) {
            self.0.push((index, plain));
        }
        fn render(&mut self, _: f32) -> Frame<'_> {
            Frame { pixels: &[0], width: 1, height: 1 }
        }
    }

    #[test]
    fn mirror_reports_everything_once_then_only_changes() {
        let mut mirror = ValueMirror::new(3);
        let mut probe = Probe(Vec::new());
        let values = std::cell::RefCell::new([1.0f32, 2.0, 3.0]);
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert_eq!(probe.0.len(), 3);
        probe.0.clear();
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert!(probe.0.is_empty());
        values.borrow_mut()[1] = 5.0;
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert_eq!(probe.0, vec![(1, 5.0)]);
        mirror.invalidate();
        probe.0.clear();
        mirror.update(&mut probe, |i| Some(values.borrow()[i]));
        assert_eq!(probe.0.len(), 3);
    }

    #[test]
    fn host_set_is_a_full_gesture() {
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = log.clone();
        let host = EditorHost::new(move |g| sink.borrow_mut().push(g));
        host.set(4, 0.5);
        host.perform(4, f32::NAN);
        assert_eq!(*log.borrow(), vec![Gesture::Begin(4), Gesture::Perform(4, 0.5), Gesture::End(4)]);
    }
}
