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
}

impl EditorHost {
    pub fn new(sink: impl Fn(Gesture) + 'static) -> Self {
        Self { sink: Rc::new(sink) }
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

/// The plugin's editor. Main thread only; it is never sent to another thread.
pub trait Editor {
    /// Size in points (logical pixels). The SDK views do not resize.
    fn size(&self) -> (u32, u32);

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
