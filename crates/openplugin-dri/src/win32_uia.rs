//! UI Automation for the Windows view: what Narrator and other screen readers
//! read. The view answers `WM_GETOBJECT` with a provider for itself (a group),
//! whose children are one provider per node of
//! [`Editor::accessibility_nodes`]: a control with a name, a control type
//! (slider, check box or button), a value as text, and its place on the screen.
//!
//! - A slider has the value pattern (the text, read-only) and, when the editor
//!   gives numbers ([`Editor::accessibility_range`]), the range value pattern:
//!   minimum, maximum, value, step, and `SetValue`, which goes to
//!   [`Editor::accessibility_set_value`].
//! - A switch has the toggle pattern (`Toggle` is [`AccessAction::Press`]) and
//!   the value pattern with its text ("On", "Off").
//! - A button has the invoke pattern (`Invoke` is [`AccessAction::Press`]).
//! - `SetFocus` on a control goes to [`Editor::accessibility_focus`], and the
//!   group's `GetFocus` is the node that has the editor's keyboard focus.
//!
//! The providers hold no copy: each holds the view's window and a node index,
//! and every question asks the editor again, so the answer is current and a
//! provider a client still holds after the view is gone answers
//! `UIA_E_ELEMENTNOTAVAILABLE`. After every tick and input call the view
//! compares the nodes with what it last saw and, when a client is listening,
//! raises a property change for a value that changed (from the host's
//! automation too), a focus change, and a structure change when the controls
//! or the size changed.
//!
//! Threads: the providers ask for COM threading, so calls arrive on the view's
//! thread when that thread has a COM apartment, as a host's window thread has.
//! A call that arrives on another thread is answered "not available": the
//! editor is never touched off its thread.
//!
//! COM by hand: each provider is one allocation that starts with one vtable
//! pointer per interface; an interface pointer is the address of its slot.

use super::*;
use openplugin_core::editor::{AccessAction, AccessNode, AccessRange, AccessRole};
use std::sync::atomic::AtomicU32;

type Hresult = i32;
const S_OK: Hresult = 0;
const E_NOINTERFACE: Hresult = 0x8000_4002u32 as i32;
const E_POINTER: Hresult = 0x8000_4003u32 as i32;
const E_INVALIDARG: Hresult = 0x8007_0057u32 as i32;
const UIA_E_ELEMENTNOTAVAILABLE: Hresult = 0x8004_0201u32 as i32;
const UIA_E_ELEMENTNOTENABLED: Hresult = 0x8004_0200u32 as i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid(u32, u16, u16, [u8; 8]);

const IID_UNKNOWN: Guid = Guid(0x0000_0000, 0x0000, 0x0000, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_SIMPLE: Guid = Guid(0xd6dd_68d1, 0x86fd, 0x4332, [0x86, 0x66, 0x9a, 0xbe, 0xde, 0xa2, 0xd2, 0x4c]);
const IID_FRAGMENT: Guid = Guid(0xf706_3da8, 0x8359, 0x439c, [0x92, 0x97, 0xbb, 0xc5, 0x29, 0x9a, 0x7d, 0x87]);
const IID_FRAGMENT_ROOT: Guid = Guid(0x620c_e2a5, 0xab8f, 0x40a9, [0x86, 0xcb, 0xde, 0x3c, 0x75, 0x59, 0x9b, 0x58]);
const IID_VALUE: Guid = Guid(0xc793_5180, 0x6fb3, 0x4201, [0xb1, 0x74, 0x7d, 0xf7, 0x3a, 0xdb, 0xf6, 0x4a]);
const IID_RANGE_VALUE: Guid = Guid(0x36dc_7aef, 0x33e6, 0x4691, [0xaf, 0xe1, 0x2b, 0xe7, 0x27, 0x4b, 0x3d, 0x33]);
const IID_TOGGLE: Guid = Guid(0x56d0_0bd0, 0xc4f4, 0x433c, [0xa8, 0x36, 0x1a, 0x52, 0xa5, 0x7e, 0x08, 0x92]);
const IID_INVOKE: Guid = Guid(0x54fc_b24b, 0xe18e, 0x47a2, [0xb4, 0xd3, 0xec, 0xcb, 0xe7, 0x75, 0x99, 0xa2]);

/// A `VARIANT`: a type tag and 16 bytes (8 on 32-bit) of value.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Variant {
    vt: u16,
    reserved: [u16; 3],
    data: [usize; 2],
}

const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;

impl Variant {
    const EMPTY: Variant = Variant { vt: VT_EMPTY, reserved: [0; 3], data: [0; 2] };

    fn i4(value: i32) -> Variant {
        let mut v = Variant { vt: VT_I4, ..Variant::EMPTY };
        unsafe { (v.data.as_mut_ptr() as *mut i32).write(value) };
        v
    }

    fn bool(value: bool) -> Variant {
        let mut v = Variant { vt: VT_BOOL, ..Variant::EMPTY };
        // VARIANT_TRUE is -1.
        unsafe { (v.data.as_mut_ptr() as *mut i16).write(if value { -1 } else { 0 }) };
        v
    }

    /// A new `BSTR`; whoever takes the variant frees it.
    fn bstr(text: &str) -> Variant {
        let mut v = Variant { vt: VT_BSTR, ..Variant::EMPTY };
        v.data[0] = bstr(text) as usize;
        v
    }
}

#[repr(C)]
struct UiaRect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

#[link(name = "uiautomationcore")]
unsafe extern "system" {
    fn UiaReturnRawElementProvider(hwnd: Hwnd, w: usize, l: isize, provider: *mut c_void) -> isize;
    fn UiaHostProviderFromHwnd(hwnd: Hwnd, provider: *mut *mut c_void) -> Hresult;
    fn UiaClientsAreListening() -> i32;
    fn UiaRaiseAutomationEvent(provider: *mut c_void, event: i32) -> Hresult;
    fn UiaRaiseAutomationPropertyChangedEvent(provider: *mut c_void, property: i32, old: Variant, new: Variant) -> Hresult;
    fn UiaRaiseStructureChangedEvent(provider: *mut c_void, change: i32, runtime_id: *mut i32, count: i32) -> Hresult;
}

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn SysAllocStringLen(text: *const u16, len: u32) -> *mut u16;
    fn SysFreeString(text: *mut u16);
    fn SafeArrayCreateVector(vt: u16, lower: i32, count: u32) -> *mut c_void;
    fn SafeArrayPutElement(array: *mut c_void, index: *const i32, value: *const c_void) -> Hresult;
}

fn bstr(text: &str) -> *mut u16 {
    let w: Vec<u16> = text.encode_utf16().collect();
    unsafe { SysAllocStringLen(w.as_ptr(), w.len() as u32) }
}

const UIA_ROOT_OBJECT_ID: isize = -25;

// Patterns, properties, control types, events (`UIAutomationClient.h`).
const PATTERN_INVOKE: i32 = 10000;
const PATTERN_VALUE: i32 = 10002;
const PATTERN_RANGE_VALUE: i32 = 10003;
const PATTERN_TOGGLE: i32 = 10015;
const PROP_CONTROL_TYPE: i32 = 30003;
const PROP_NAME: i32 = 30005;
const PROP_HAS_KEYBOARD_FOCUS: i32 = 30008;
const PROP_IS_KEYBOARD_FOCUSABLE: i32 = 30009;
const PROP_IS_ENABLED: i32 = 30010;
const PROP_AUTOMATION_ID: i32 = 30011;
const PROP_IS_CONTROL_ELEMENT: i32 = 30016;
const PROP_IS_CONTENT_ELEMENT: i32 = 30017;
const PROP_FRAMEWORK_ID: i32 = 30024;
const PROP_VALUE_VALUE: i32 = 30045;
const PROP_TOGGLE_STATE: i32 = 30086;
const TYPE_BUTTON: i32 = 50000;
const TYPE_CHECK_BOX: i32 = 50002;
const TYPE_SLIDER: i32 = 50015;
const TYPE_GROUP: i32 = 50026;
const EVENT_FOCUS_CHANGED: i32 = 20005;

/// The control type a role is read as.
fn control_type(role: AccessRole) -> i32 {
    match role {
        AccessRole::Slider => TYPE_SLIDER,
        AccessRole::Switch => TYPE_CHECK_BOX,
        _ => TYPE_BUTTON,
    }
}

/// The patterns a node offers.
fn has_pattern(node: &AccessNode, ranged: bool, pattern: i32) -> bool {
    match (node.role, pattern) {
        (AccessRole::Slider, PATTERN_VALUE) | (AccessRole::Switch, PATTERN_VALUE | PATTERN_TOGGLE) | (AccessRole::Button, PATTERN_INVOKE) => true,
        (AccessRole::Slider, PATTERN_RANGE_VALUE) => ranged,
        _ => false,
    }
}

/// A node as the last event pass saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Seen {
    value: String,
    on: bool,
    focused: bool,
}

/// What changed between two passes over the nodes.
#[derive(Debug, PartialEq, Eq)]
struct Diff {
    /// The number of nodes changed.
    layout: bool,
    /// Nodes whose value text or on state changed.
    values: Vec<usize>,
    /// The focused node changed: the new one (`None` for no focus).
    focus: Option<Option<usize>>,
}

fn diff(prev: &[Seen], now: &[AccessNode]) -> Diff {
    let values = now.iter().enumerate().filter(|(i, n)| prev.get(*i).is_some_and(|p| p.value != n.value || p.on != n.on)).map(|(i, _)| i).collect();
    let before = prev.iter().position(|p| p.focused);
    let after = now.iter().position(|n| n.focused);
    Diff { layout: prev.len() != now.len(), values, focus: (before != after).then_some(after) }
}

// ---- The provider object -----------------------------------------------------

/// Interfaces in the order of their vtable slots in [`Provider`].
const SLOT_SIMPLE: usize = 0;
const SLOT_FRAGMENT: usize = 1;
const SLOT_ROOT: usize = 2;
const SLOT_VALUE: usize = 3;
const SLOT_RANGE: usize = 4;
const SLOT_TOGGLE: usize = 5;
const SLOT_INVOKE: usize = 6;
const SLOTS: usize = 7;

#[repr(C)]
struct Provider {
    /// One vtable pointer per interface; an interface pointer points at its slot.
    vtables: [*const c_void; SLOTS],
    refs: AtomicU32,
    /// The view's window. The state is looked up from it at every call.
    view: Hwnd,
    /// The node, or `None` for the view itself.
    node: Option<usize>,
}

type This = *mut c_void;

/// The provider an interface pointer of slot `SLOT` belongs to.
unsafe fn provider<'a, const SLOT: usize>(this: This) -> &'a Provider {
    unsafe { &*((this as *const *const c_void).sub(SLOT) as *const Provider) }
}

unsafe fn interface(p: *const Provider, slot: usize) -> *mut c_void {
    unsafe { (p as *const *const c_void).add(slot) as *mut c_void }
}

fn new_provider(view: Hwnd, node: Option<usize>) -> *mut Provider {
    Box::into_raw(Box::new(Provider {
        vtables: [
            &SIMPLE_VTABLE as *const SimpleVtable as *const c_void,
            &FRAGMENT_VTABLE as *const FragmentVtable as *const c_void,
            &ROOT_VTABLE as *const RootVtable as *const c_void,
            &VALUE_VTABLE as *const ValueVtable as *const c_void,
            &RANGE_VTABLE as *const RangeVtable as *const c_void,
            &TOGGLE_VTABLE as *const ToggleVtable as *const c_void,
            &INVOKE_VTABLE as *const InvokeVtable as *const c_void,
        ],
        refs: AtomicU32::new(1),
        view,
        node,
    }))
}

unsafe extern "system" fn query<const SLOT: usize>(this: This, iid: *const Guid, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() || iid.is_null() {
            return E_POINTER;
        }
        let p = provider::<SLOT>(this);
        let child = p.node.is_some();
        let slot = match *iid {
            IID_UNKNOWN | IID_SIMPLE => Some(SLOT_SIMPLE),
            IID_FRAGMENT => Some(SLOT_FRAGMENT),
            IID_FRAGMENT_ROOT if !child => Some(SLOT_ROOT),
            IID_VALUE if child => Some(SLOT_VALUE),
            IID_RANGE_VALUE if child => Some(SLOT_RANGE),
            IID_TOGGLE if child => Some(SLOT_TOGGLE),
            IID_INVOKE if child => Some(SLOT_INVOKE),
            _ => None,
        };
        match slot {
            Some(slot) => {
                p.refs.fetch_add(1, Ordering::SeqCst);
                *out = interface(p, slot);
                S_OK
            }
            None => {
                *out = null_mut();
                E_NOINTERFACE
            }
        }
    }
}

unsafe extern "system" fn add_ref<const SLOT: usize>(this: This) -> u32 {
    unsafe { provider::<SLOT>(this).refs.fetch_add(1, Ordering::SeqCst) + 1 }
}

unsafe extern "system" fn release<const SLOT: usize>(this: This) -> u32 {
    unsafe {
        let p = provider::<SLOT>(this);
        let left = p.refs.fetch_sub(1, Ordering::SeqCst) - 1;
        if left == 0 {
            drop(Box::from_raw(p as *const Provider as *mut Provider));
        }
        left
    }
}

#[repr(C)]
struct Unknown {
    query: unsafe extern "system" fn(This, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(This) -> u32,
    release: unsafe extern "system" fn(This) -> u32,
}

const fn unknown<const SLOT: usize>() -> Unknown {
    Unknown { query: query::<SLOT>, add_ref: add_ref::<SLOT>, release: release::<SLOT> }
}

/// The view's state, if the window is still the view's and the editor can be asked now.
unsafe fn state_for<'a>(p: &Provider) -> Option<&'a mut State> {
    unsafe {
        if p.view.is_null() || IsWindow(p.view) == 0 {
            return None;
        }
        // The editor lives on the view's thread. UI Automation calls there when that
        // thread has a COM apartment (a host's does); a call from another thread is refused.
        if GetWindowThreadProcessId(p.view, null_mut()) != GetCurrentThreadId() {
            return None;
        }
        let sp = state_of(p.view);
        if sp.is_null() || (*sp).busy || (*sp).hwnd != p.view {
            return None;
        }
        Some(&mut *sp)
    }
}

/// The provider's node, as the editor has it now, with the view's state.
unsafe fn node_for<'a>(p: &Provider) -> Option<(&'a mut State, usize, AccessNode)> {
    unsafe {
        let index = p.node?;
        let st = state_for(p)?;
        let node = st.editor.accessibility_nodes().into_iter().nth(index)?;
        Some((st, index, node))
    }
}

/// Run a call that changes the editor (an action, a value, focus), then let the
/// view repaint and tell the listeners. False when the editor is busy or gone.
unsafe fn act(p: &Provider, call: impl FnOnce(&mut State) -> bool) -> bool {
    unsafe {
        let Some(st) = state_for(p) else { return false };
        let sp = st as *mut State;
        let mut done = false;
        guarded(sp, |st| done = call(st));
        if alive(p.view, sp) {
            invalidate(sp);
        }
        done
    }
}

// ---- IRawElementProviderSimple ------------------------------------------------

#[repr(C)]
struct SimpleVtable {
    unknown: Unknown,
    provider_options: unsafe extern "system" fn(This, *mut i32) -> Hresult,
    pattern_provider: unsafe extern "system" fn(This, i32, *mut *mut c_void) -> Hresult,
    property_value: unsafe extern "system" fn(This, i32, *mut Variant) -> Hresult,
    host_provider: unsafe extern "system" fn(This, *mut *mut c_void) -> Hresult,
}

static SIMPLE_VTABLE: SimpleVtable = SimpleVtable { unknown: unknown::<SLOT_SIMPLE>(), provider_options, pattern_provider, property_value, host_provider };

unsafe extern "system" fn provider_options(_this: This, out: *mut i32) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading: the
    // calls arrive on the thread that made the provider, the view's.
    unsafe { *out = 0x1 | 0x20 };
    S_OK
}

unsafe extern "system" fn pattern_provider(this: This, pattern: i32, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = null_mut();
        let p = provider::<SLOT_SIMPLE>(this);
        if p.node.is_none() {
            return S_OK;
        }
        let Some((st, index, node)) = node_for(p) else { return UIA_E_ELEMENTNOTAVAILABLE };
        let ranged = st.editor.accessibility_range(index).is_some();
        if has_pattern(&node, ranged, pattern) {
            let slot = match pattern {
                PATTERN_VALUE => SLOT_VALUE,
                PATTERN_RANGE_VALUE => SLOT_RANGE,
                PATTERN_TOGGLE => SLOT_TOGGLE,
                _ => SLOT_INVOKE,
            };
            p.refs.fetch_add(1, Ordering::SeqCst);
            *out = interface(p, slot);
        }
        S_OK
    }
}

unsafe extern "system" fn property_value(this: This, property: i32, out: *mut Variant) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = Variant::EMPTY;
        let p = provider::<SLOT_SIMPLE>(this);
        if p.node.is_none() {
            // The window's own provider answers the rest (the place, the focus, the handle).
            *out = match property {
                PROP_CONTROL_TYPE => Variant::i4(TYPE_GROUP),
                PROP_FRAMEWORK_ID => Variant::bstr("OpenPlugin"),
                PROP_IS_CONTROL_ELEMENT | PROP_IS_CONTENT_ELEMENT => Variant::bool(true),
                _ => Variant::EMPTY,
            };
            return S_OK;
        }
        let Some((_, index, node)) = node_for(p) else { return UIA_E_ELEMENTNOTAVAILABLE };
        *out = match property {
            PROP_CONTROL_TYPE => Variant::i4(control_type(node.role)),
            PROP_NAME => Variant::bstr(&node.label),
            PROP_AUTOMATION_ID => Variant::bstr(&format!("node{index}")),
            PROP_FRAMEWORK_ID => Variant::bstr("OpenPlugin"),
            PROP_IS_ENABLED => Variant::bool(node.enabled),
            PROP_IS_KEYBOARD_FOCUSABLE => Variant::bool(node.enabled),
            PROP_HAS_KEYBOARD_FOCUS => Variant::bool(node.focused),
            PROP_IS_CONTROL_ELEMENT | PROP_IS_CONTENT_ELEMENT => Variant::bool(true),
            _ => Variant::EMPTY,
        };
        S_OK
    }
}

unsafe extern "system" fn host_provider(this: This, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = null_mut();
        let p = provider::<SLOT_SIMPLE>(this);
        if p.node.is_none() && IsWindow(p.view) != 0 {
            return UiaHostProviderFromHwnd(p.view, out);
        }
        S_OK
    }
}

// ---- IRawElementProviderFragment ----------------------------------------------

#[repr(C)]
struct FragmentVtable {
    unknown: Unknown,
    navigate: unsafe extern "system" fn(This, i32, *mut *mut c_void) -> Hresult,
    runtime_id: unsafe extern "system" fn(This, *mut *mut c_void) -> Hresult,
    bounding_rectangle: unsafe extern "system" fn(This, *mut UiaRect) -> Hresult,
    embedded_roots: unsafe extern "system" fn(This, *mut *mut c_void) -> Hresult,
    set_focus: unsafe extern "system" fn(This) -> Hresult,
    fragment_root: unsafe extern "system" fn(This, *mut *mut c_void) -> Hresult,
}

static FRAGMENT_VTABLE: FragmentVtable =
    FragmentVtable { unknown: unknown::<SLOT_FRAGMENT>(), navigate, runtime_id, bounding_rectangle, embedded_roots, set_focus, fragment_root };

/// A new provider for node `index` of the view, as a fragment.
unsafe fn child_fragment(view: Hwnd, index: usize) -> *mut c_void {
    unsafe { interface(new_provider(view, Some(index)), SLOT_FRAGMENT) }
}

unsafe extern "system" fn navigate(this: This, direction: i32, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = null_mut();
        let p = provider::<SLOT_FRAGMENT>(this);
        let Some(st) = state_for(p) else { return UIA_E_ELEMENTNOTAVAILABLE };
        let count = st.editor.accessibility_nodes().len();
        // Parent 0, NextSibling 1, PreviousSibling 2, FirstChild 3, LastChild 4.
        let target = match (p.node, direction) {
            (None, 3) if count > 0 => Some(Some(0)),
            (None, 4) if count > 0 => Some(Some(count - 1)),
            (Some(i), 0) if i < count => Some(None),
            (Some(i), 1) if i + 1 < count => Some(Some(i + 1)),
            (Some(i), 2) if i > 0 && i < count => Some(Some(i - 1)),
            _ => None,
        };
        match target {
            Some(Some(index)) => *out = child_fragment(p.view, index),
            Some(None) => *out = interface(new_provider(p.view, None), SLOT_FRAGMENT),
            None => {}
        }
        S_OK
    }
}

unsafe extern "system" fn runtime_id(this: This, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = null_mut();
        let p = provider::<SLOT_FRAGMENT>(this);
        // The window's provider names the view; a node is the window's id and its index.
        let Some(index) = p.node else { return S_OK };
        // UiaAppendRuntimeId, then what is unique under this window.
        let ids = [3i32, index as i32 + 1];
        let array = SafeArrayCreateVector(VT_I4, 0, ids.len() as u32);
        if array.is_null() {
            return E_INVALIDARG;
        }
        for (i, id) in ids.iter().enumerate() {
            let at = i as i32;
            SafeArrayPutElement(array, &at, id as *const i32 as *const c_void);
        }
        *out = array;
        S_OK
    }
}

/// The node's rectangle on the screen, in pixels.
unsafe fn screen_rect(st: &State, node: &AccessNode) -> UiaRect {
    unsafe {
        let r = view_space(st, &node.rect);
        let mut origin = Point { x: r.left, y: r.top };
        ClientToScreen(st.hwnd, &mut origin);
        UiaRect { left: f64::from(origin.x), top: f64::from(origin.y), width: f64::from(r.right - r.left), height: f64::from(r.bottom - r.top) }
    }
}

unsafe extern "system" fn bounding_rectangle(this: This, out: *mut UiaRect) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = UiaRect { left: 0.0, top: 0.0, width: 0.0, height: 0.0 };
        let p = provider::<SLOT_FRAGMENT>(this);
        if p.node.is_none() {
            // The window's provider knows the window's place.
            return S_OK;
        }
        let Some((st, _, node)) = node_for(p) else { return UIA_E_ELEMENTNOTAVAILABLE };
        *out = screen_rect(st, &node);
        S_OK
    }
}

unsafe extern "system" fn embedded_roots(_this: This, out: *mut *mut c_void) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = null_mut() };
    S_OK
}

unsafe extern "system" fn set_focus(this: This) -> Hresult {
    unsafe {
        let p = provider::<SLOT_FRAGMENT>(this);
        let Some(index) = p.node else { return S_OK };
        if node_for(p).is_none() {
            return UIA_E_ELEMENTNOTAVAILABLE;
        }
        // The keyboard goes to the view's window, then to the control in it.
        if GetFocus() != p.view {
            SetFocus(p.view);
        }
        act(p, |st| st.editor.accessibility_focus(index));
        S_OK
    }
}

unsafe extern "system" fn fragment_root(this: This, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        let p = provider::<SLOT_FRAGMENT>(this);
        *out = match p.node {
            None => {
                p.refs.fetch_add(1, Ordering::SeqCst);
                interface(p, SLOT_ROOT)
            }
            Some(_) => interface(new_provider(p.view, None), SLOT_ROOT),
        };
        S_OK
    }
}

// ---- IRawElementProviderFragmentRoot ------------------------------------------

#[repr(C)]
struct RootVtable {
    unknown: Unknown,
    from_point: unsafe extern "system" fn(This, f64, f64, *mut *mut c_void) -> Hresult,
    get_focus: unsafe extern "system" fn(This, *mut *mut c_void) -> Hresult,
}

static ROOT_VTABLE: RootVtable = RootVtable { unknown: unknown::<SLOT_ROOT>(), from_point, get_focus };

/// The node at a screen position: the last one whose rectangle holds it.
fn node_at(rects: &[UiaRect], x: f64, y: f64) -> Option<usize> {
    rects.iter().rposition(|r| x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height)
}

unsafe extern "system" fn from_point(this: This, x: f64, y: f64, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = null_mut();
        let p = provider::<SLOT_ROOT>(this);
        let Some(st) = state_for(p) else { return UIA_E_ELEMENTNOTAVAILABLE };
        let rects: Vec<UiaRect> = st.editor.accessibility_nodes().iter().map(|n| screen_rect(st, n)).collect();
        *out = match node_at(&rects, x, y) {
            Some(index) => child_fragment(p.view, index),
            // Bare plate: the view itself.
            None => {
                p.refs.fetch_add(1, Ordering::SeqCst);
                interface(p, SLOT_FRAGMENT)
            }
        };
        S_OK
    }
}

unsafe extern "system" fn get_focus(this: This, out: *mut *mut c_void) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = null_mut();
        let p = provider::<SLOT_ROOT>(this);
        let Some(st) = state_for(p) else { return UIA_E_ELEMENTNOTAVAILABLE };
        if let Some(index) = st.editor.accessibility_nodes().iter().position(|n| n.focused) {
            *out = child_fragment(p.view, index);
        }
        S_OK
    }
}

// ---- IValueProvider -------------------------------------------------------------

#[repr(C)]
struct ValueVtable {
    unknown: Unknown,
    set_value: unsafe extern "system" fn(This, *const u16) -> Hresult,
    value: unsafe extern "system" fn(This, *mut *mut u16) -> Hresult,
    is_read_only: unsafe extern "system" fn(This, *mut i32) -> Hresult,
}

static VALUE_VTABLE: ValueVtable = ValueVtable { unknown: unknown::<SLOT_VALUE>(), set_value: value_set, value: value_get, is_read_only: value_read_only };

/// The text is what the editor formats; a number is set through the range.
unsafe extern "system" fn value_set(_this: This, _text: *const u16) -> Hresult {
    UIA_E_ELEMENTNOTENABLED
}

unsafe extern "system" fn value_get(this: This, out: *mut *mut u16) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = null_mut();
        let Some((_, _, node)) = node_for(provider::<SLOT_VALUE>(this)) else { return UIA_E_ELEMENTNOTAVAILABLE };
        *out = bstr(&node.value);
        S_OK
    }
}

unsafe extern "system" fn value_read_only(_this: This, out: *mut i32) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = 1 };
    S_OK
}

// ---- IRangeValueProvider --------------------------------------------------------

#[repr(C)]
struct RangeVtable {
    unknown: Unknown,
    set_value: unsafe extern "system" fn(This, f64) -> Hresult,
    value: unsafe extern "system" fn(This, *mut f64) -> Hresult,
    is_read_only: unsafe extern "system" fn(This, *mut i32) -> Hresult,
    maximum: unsafe extern "system" fn(This, *mut f64) -> Hresult,
    minimum: unsafe extern "system" fn(This, *mut f64) -> Hresult,
    large_change: unsafe extern "system" fn(This, *mut f64) -> Hresult,
    small_change: unsafe extern "system" fn(This, *mut f64) -> Hresult,
}

static RANGE_VTABLE: RangeVtable = RangeVtable {
    unknown: unknown::<SLOT_RANGE>(),
    set_value: range_set,
    value: range_value,
    is_read_only: range_read_only,
    maximum: range_maximum,
    minimum: range_minimum,
    large_change: range_large,
    small_change: range_small,
};

unsafe fn range_for(this: This) -> Option<(AccessRange, AccessNode)> {
    unsafe {
        let (st, index, node) = node_for(provider::<SLOT_RANGE>(this))?;
        Some((st.editor.accessibility_range(index)?, node))
    }
}

unsafe fn range_number(this: This, out: *mut f64, pick: impl FnOnce(&AccessRange) -> f64) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = 0.0;
        let Some((range, _)) = range_for(this) else { return UIA_E_ELEMENTNOTAVAILABLE };
        *out = pick(&range);
        S_OK
    }
}

unsafe extern "system" fn range_set(this: This, value: f64) -> Hresult {
    unsafe {
        let p = provider::<SLOT_RANGE>(this);
        let Some((range, node)) = range_for(this) else { return UIA_E_ELEMENTNOTAVAILABLE };
        if !node.enabled {
            return UIA_E_ELEMENTNOTENABLED;
        }
        if !value.is_finite() || value < range.min || value > range.max {
            return E_INVALIDARG;
        }
        let Some(index) = p.node else { return E_INVALIDARG };
        act(p, |st| st.editor.accessibility_set_value(index, value));
        S_OK
    }
}

unsafe extern "system" fn range_value(this: This, out: *mut f64) -> Hresult {
    unsafe { range_number(this, out, |r| r.value) }
}

unsafe extern "system" fn range_maximum(this: This, out: *mut f64) -> Hresult {
    unsafe { range_number(this, out, |r| r.max) }
}

unsafe extern "system" fn range_minimum(this: This, out: *mut f64) -> Hresult {
    unsafe { range_number(this, out, |r| r.min) }
}

unsafe extern "system" fn range_small(this: This, out: *mut f64) -> Hresult {
    unsafe { range_number(this, out, |r| r.step) }
}

unsafe extern "system" fn range_large(this: This, out: *mut f64) -> Hresult {
    unsafe { range_number(this, out, |r| r.step) }
}

unsafe extern "system" fn range_read_only(this: This, out: *mut i32) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = 1;
        let Some((_, node)) = range_for(this) else { return UIA_E_ELEMENTNOTAVAILABLE };
        *out = i32::from(!node.enabled);
        S_OK
    }
}

// ---- IToggleProvider, IInvokeProvider -------------------------------------------

#[repr(C)]
struct ToggleVtable {
    unknown: Unknown,
    toggle: unsafe extern "system" fn(This) -> Hresult,
    state: unsafe extern "system" fn(This, *mut i32) -> Hresult,
}

static TOGGLE_VTABLE: ToggleVtable = ToggleVtable { unknown: unknown::<SLOT_TOGGLE>(), toggle: toggle_press, state: toggle_state };

#[repr(C)]
struct InvokeVtable {
    unknown: Unknown,
    invoke: unsafe extern "system" fn(This) -> Hresult,
}

static INVOKE_VTABLE: InvokeVtable = InvokeVtable { unknown: unknown::<SLOT_INVOKE>(), invoke: invoke_press };

/// Press the node: flip a switch, push a button.
unsafe fn press(p: &Provider) -> Hresult {
    unsafe {
        let Some((_, index, node)) = node_for(p) else { return UIA_E_ELEMENTNOTAVAILABLE };
        if !node.enabled {
            return UIA_E_ELEMENTNOTENABLED;
        }
        act(p, |st| st.editor.accessibility_action(index, AccessAction::Press));
        S_OK
    }
}

unsafe extern "system" fn toggle_press(this: This) -> Hresult {
    unsafe { press(provider::<SLOT_TOGGLE>(this)) }
}

unsafe extern "system" fn invoke_press(this: This) -> Hresult {
    unsafe { press(provider::<SLOT_INVOKE>(this)) }
}

unsafe extern "system" fn toggle_state(this: This, out: *mut i32) -> Hresult {
    unsafe {
        if out.is_null() {
            return E_POINTER;
        }
        *out = 0;
        let Some((_, _, node)) = node_for(provider::<SLOT_TOGGLE>(this)) else { return UIA_E_ELEMENTNOTAVAILABLE };
        // ToggleState_Off 0, ToggleState_On 1.
        *out = i32::from(node.on);
        S_OK
    }
}

// ---- What the view calls ---------------------------------------------------------

/// `WM_GETOBJECT`: the view's provider for a UI Automation client, or `None`
/// for a request that is not UI Automation's (the default handling answers it).
pub(super) unsafe fn get_object(hwnd: Hwnd, w: usize, l: isize) -> Option<isize> {
    // The id is a 32-bit value in the low half of the parameter.
    if l as i32 as isize != UIA_ROOT_OBJECT_ID {
        return None;
    }
    unsafe {
        let p = new_provider(hwnd, None);
        let result = UiaReturnRawElementProvider(hwnd, w, l, interface(p, SLOT_SIMPLE));
        release::<SLOT_SIMPLE>(interface(p, SLOT_SIMPLE));
        Some(result)
    }
}

/// The window is going: providers that clients still hold are cut off from it.
pub(super) unsafe fn disconnect(hwnd: Hwnd) {
    unsafe { UiaReturnRawElementProvider(hwnd, 0, 0, null_mut()) };
}

/// Tell the listeners what changed since the last pass: values, focus, layout.
/// Never called inside an editor call.
pub(super) unsafe fn sync(sp: *mut State) {
    unsafe {
        let st = &mut *sp;
        if st.busy || st.hwnd.is_null() {
            return;
        }
        // Nobody listens: nothing is asked of the editor, and the next listener starts clean.
        if UiaClientsAreListening() == 0 {
            st.uia_seen.clear();
            return;
        }
        let nodes = st.editor.accessibility_nodes();
        let d = diff(&st.uia_seen, &nodes);
        let was = std::mem::replace(&mut st.uia_seen, nodes.iter().map(|n| Seen { value: n.value.clone(), on: n.on, focused: n.focused }).collect());
        let hwnd = st.hwnd;
        if d.layout {
            let root = new_provider(hwnd, None);
            // StructureChangeType_ChildrenInvalidated
            UiaRaiseStructureChangedEvent(interface(root, SLOT_SIMPLE), 2, null_mut(), 0);
            release::<SLOT_SIMPLE>(interface(root, SLOT_SIMPLE));
        }
        for i in d.values {
            let child = new_provider(hwnd, Some(i));
            let simple = interface(child, SLOT_SIMPLE);
            let (old, new) = (Variant::bstr(&was[i].value), Variant::bstr(&nodes[i].value));
            UiaRaiseAutomationPropertyChangedEvent(simple, PROP_VALUE_VALUE, old, new);
            SysFreeString(old.data[0] as *mut u16);
            SysFreeString(new.data[0] as *mut u16);
            if nodes[i].role == AccessRole::Switch && was[i].on != nodes[i].on {
                UiaRaiseAutomationPropertyChangedEvent(simple, PROP_TOGGLE_STATE, Variant::i4(i32::from(was[i].on)), Variant::i4(i32::from(nodes[i].on)));
            }
            release::<SLOT_SIMPLE>(simple);
        }
        if let Some(Some(i)) = d.focus {
            let child = new_provider(hwnd, Some(i));
            UiaRaiseAutomationEvent(interface(child, SLOT_SIMPLE), EVENT_FOCUS_CHANGED);
            release::<SLOT_SIMPLE>(interface(child, SLOT_SIMPLE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin_core::editor::DamageRect;

    fn node(role: AccessRole, value: &str, on: bool, focused: bool) -> AccessNode {
        AccessNode { label: "x".into(), value: value.into(), role, on, rect: DamageRect::new(0.0, 0.0, 1.0, 1.0), focused, enabled: true }
    }

    #[test]
    fn roles_map_to_control_types_and_patterns() {
        assert_eq!(control_type(AccessRole::Slider), 50015);
        assert_eq!(control_type(AccessRole::Switch), 50002);
        assert_eq!(control_type(AccessRole::Button), 50000);
        let slider = node(AccessRole::Slider, "1", false, false);
        assert!(has_pattern(&slider, true, PATTERN_VALUE) && has_pattern(&slider, true, PATTERN_RANGE_VALUE));
        assert!(!has_pattern(&slider, false, PATTERN_RANGE_VALUE), "no numbers from the editor, no range");
        assert!(!has_pattern(&slider, true, PATTERN_TOGGLE) && !has_pattern(&slider, true, PATTERN_INVOKE));
        let switch = node(AccessRole::Switch, "On", true, false);
        assert!(has_pattern(&switch, false, PATTERN_TOGGLE) && has_pattern(&switch, false, PATTERN_VALUE));
        assert!(!has_pattern(&switch, true, PATTERN_RANGE_VALUE) && !has_pattern(&switch, false, PATTERN_INVOKE));
        let button = node(AccessRole::Button, "", false, false);
        assert!(has_pattern(&button, false, PATTERN_INVOKE) && !has_pattern(&button, false, PATTERN_VALUE));
    }

    #[test]
    fn diff_reports_values_toggles_focus_and_layout() {
        let seen = |value: &str, on: bool, focused: bool| Seen { value: value.into(), on, focused };
        let prev = [seen("1", false, false), seen("Off", false, false)];
        let d = diff(&prev, &[node(AccessRole::Slider, "1", false, false), node(AccessRole::Switch, "Off", false, false)]);
        assert_eq!(d, Diff { layout: false, values: vec![], focus: None });
        let d = diff(&prev, &[node(AccessRole::Slider, "2", false, false), node(AccessRole::Switch, "On", true, true)]);
        assert_eq!(d, Diff { layout: false, values: vec![0, 1], focus: Some(Some(1)) });
        let d = diff(&[seen("1", false, true)], &[node(AccessRole::Slider, "1", false, false)]);
        assert_eq!(d.focus, Some(None));
        let d = diff(&[], &[node(AccessRole::Slider, "1", false, false)]);
        assert!(d.layout && d.values.is_empty());
    }

    #[test]
    fn a_point_finds_the_node_under_it() {
        let rects = [UiaRect { left: 10.0, top: 10.0, width: 50.0, height: 50.0 }, UiaRect { left: 100.0, top: 10.0, width: 20.0, height: 40.0 }];
        assert_eq!(node_at(&rects, 20.0, 20.0), Some(0));
        assert_eq!(node_at(&rects, 119.0, 49.0), Some(1));
        assert_eq!(node_at(&rects, 120.0, 20.0), None, "the far edge is outside");
        assert_eq!(node_at(&rects, 5.0, 5.0), None);
    }

    #[test]
    fn variants_carry_their_types() {
        let v = Variant::i4(50015);
        assert_eq!((v.vt, unsafe { *(v.data.as_ptr() as *const i32) }), (VT_I4, 50015));
        let v = Variant::bool(true);
        assert_eq!((v.vt, unsafe { *(v.data.as_ptr() as *const i16) }), (VT_BOOL, -1));
        assert_eq!(std::mem::size_of::<Variant>(), 8 + 2 * std::mem::size_of::<usize>());
        let v = Variant::bstr("Gain");
        assert_eq!(v.vt, VT_BSTR);
        let text = unsafe { std::slice::from_raw_parts(v.data[0] as *const u16, 4) };
        assert_eq!(String::from_utf16_lossy(text), "Gain");
        unsafe { SysFreeString(v.data[0] as *mut u16) };
    }

    #[test]
    fn a_provider_answers_for_its_interfaces_and_frees_itself() {
        unsafe {
            let p = new_provider(null_mut(), Some(0));
            let simple = interface(p, SLOT_SIMPLE);
            let mut out = null_mut();
            assert_eq!(query::<SLOT_SIMPLE>(simple, &IID_TOGGLE, &mut out), S_OK);
            assert_eq!(out, interface(p, SLOT_TOGGLE));
            // From any interface back to the first, as COM identity needs.
            let mut back = null_mut();
            assert_eq!(query::<SLOT_TOGGLE>(out, &IID_UNKNOWN, &mut back), S_OK);
            assert_eq!(back, simple);
            assert_eq!(query::<SLOT_SIMPLE>(simple, &IID_FRAGMENT_ROOT, &mut back), E_NOINTERFACE, "a node is not a root");
            assert!(back.is_null());
            // No window behind it: every question says the element is gone.
            let mut state = 9;
            assert_eq!(toggle_state(out, &mut state), UIA_E_ELEMENTNOTAVAILABLE);
            assert_eq!(release::<SLOT_TOGGLE>(out), 2);
            assert_eq!(release::<SLOT_SIMPLE>(simple), 1);
            assert_eq!(release::<SLOT_SIMPLE>(simple), 0);
            let root = new_provider(null_mut(), None);
            assert_eq!(query::<SLOT_SIMPLE>(interface(root, SLOT_SIMPLE), &IID_VALUE, &mut back), E_NOINTERFACE, "the view has no value");
            assert_eq!(query::<SLOT_SIMPLE>(interface(root, SLOT_SIMPLE), &IID_FRAGMENT_ROOT, &mut back), S_OK);
            assert_eq!(release::<SLOT_ROOT>(back), 1);
            assert_eq!(release::<SLOT_SIMPLE>(interface(root, SLOT_SIMPLE)), 0);
        }
    }
}
