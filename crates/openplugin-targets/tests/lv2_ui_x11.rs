//! The LV2 `ui:X11UI` on Linux: the description files, the exported UI
//! descriptor and its extension data, and instantiation failing cleanly
//! (null, every editor it made dropped) without a `ui:parent` window or
//! when the X11 view cannot be made. Needs no X server.
#![cfg(target_os = "linux")]

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use openplugin_core::editor::SizeRange;
use openplugin_core::{Category, Editor, EditorHost, Frame, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::lv2::{self, Lv2Feature, Lv2UiDescriptor};

static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];

static INFO: PluginInfo = PluginInfo::new(
    "urn:openplugin:test-x11-ui",
    "X11 UI Test",
    "OpenGrid",
    "0.1.0",
    Category::Utility,
    &PARAMS,
)
.with_editor(true);

#[derive(Default)]
struct Probe;

impl Plugin for Probe {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
}

static MADE: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicUsize = AtomicUsize::new(0);
/// Tests that count editors run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

struct Counted {
    pixels: Vec<u32>,
    size: (u32, u32),
}

impl Drop for Counted {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}

impl Editor for Counted {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn size_range(&self) -> SizeRange {
        SizeRange::between((100, 50), (400, 200))
    }
    fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
    }
    fn param_changed(&mut self, _: usize, _: f32) {}
    fn render(&mut self, _scale: f32) -> Frame<'_> {
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        self.pixels.resize(w * h, 0);
        Frame::new(&self.pixels, w, h)
    }
}

fn make_editor(_host: EditorHost) -> Option<Box<dyn Editor>> {
    MADE.fetch_add(1, Ordering::SeqCst);
    Some(Box::new(Counted { pixels: Vec::new(), size: (200, 100) }))
}

fn fixed_editor(_host: EditorHost) -> Option<Box<dyn Editor>> {
    struct Fixed;
    impl Editor for Fixed {
        fn size(&self) -> (u32, u32) {
            (120, 80)
        }
        fn param_changed(&mut self, _: usize, _: f32) {}
        fn render(&mut self, _scale: f32) -> Frame<'_> {
            Frame::new(&[], 0, 0)
        }
    }
    Some(Box::new(Fixed))
}

/// Plugins the UI entry constructed.
static BUILT: AtomicUsize = AtomicUsize::new(0);

fn install() -> *const Lv2UiDescriptor {
    lv2::set_creator(|| {
        BUILT.fetch_add(1, Ordering::SeqCst);
        Box::new(Probe)
    });
    lv2::set_editor(make_editor);
    lv2::lv2ui_descriptor(0)
}

type IdleFn = unsafe extern "C" fn(*mut c_void) -> i32;
type ResizeFn = unsafe extern "C" fn(*mut c_void, i32, i32) -> i32;

#[repr(C)]
struct Resize {
    handle: *mut c_void,
    ui_resize: Option<ResizeFn>,
}

fn ext(desc: *const Lv2UiDescriptor, uri: &CStr) -> *const c_void {
    unsafe { ((*desc).extension_data)(uri.as_ptr()) }
}

#[test]
fn ttl_describes_an_x11_ui_driven_by_idle() {
    let resizable = lv2::plugin_ttl_sized(&Probe, Some(true));
    let ui = &resizable[resizable.find("#ui>\n").expect("ui subject")..];
    assert!(ui.contains("a ui:X11UI ;"));
    assert!(ui.contains("lv2:requiredFeature ui:idleInterface , ui:parent ;"));
    assert!(ui.contains("lv2:extensionData ui:idleInterface ;"));
    assert!(ui.contains("lv2:extensionData ui:resize ."));
    assert!(ui.contains("ui:resize") && !ui.contains("noUserResize"));
    assert!(!resizable.contains("CocoaUI"));
    assert!(resizable.contains("ui:ui <urn:openplugin:test-x11-ui#ui> ;"));
    assert!(resizable.trim_end().ends_with('.'));

    let fixed = lv2::plugin_ttl_sized(&Probe, Some(false));
    assert!(fixed.contains("a ui:X11UI ;") && fixed.contains("ui:noUserResize"));
    assert!(fixed.contains("lv2:extensionData ui:idleInterface .") && !fixed.contains("lv2:extensionData ui:resize"));
    assert!(!fixed.contains("CocoaUI"));

    let none = lv2::plugin_ttl_sized(&Probe, None);
    assert!(!none.contains("X11UI") && !none.contains("#ui>"));

    let manifest = lv2::manifest_ttl(&Probe, "libprobe", true);
    assert!(manifest.contains("<urn:openplugin:test-x11-ui#ui>\n    a ui:X11UI ;\n    ui:binary <libprobe.so> ;"));
    assert!(!manifest.contains("CocoaUI"));
    assert!(!lv2::manifest_ttl(&Probe, "libprobe", false).contains("X11UI"));
}

#[test]
fn bundles_describe_the_x11_ui() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("op-lv2-x11ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    lv2::write_bundle(&Probe, &dir, "libprobe", true).unwrap();
    assert!(std::fs::read_to_string(dir.join("manifest.ttl")).unwrap().contains("a ui:X11UI"));
    assert!(std::fs::read_to_string(dir.join("libprobe.ttl")).unwrap().contains("ui:noUserResize"));

    lv2::write_bundle_for_editor(&Probe, &dir, "libprobe", Some(make_editor)).unwrap();
    let ttl = std::fs::read_to_string(dir.join("libprobe.ttl")).unwrap();
    assert!(ttl.contains("a ui:X11UI") && ttl.contains("lv2:extensionData ui:resize ."));
    lv2::write_bundle_for_editor(&Probe, &dir, "libprobe", Some(fixed_editor)).unwrap();
    assert!(std::fs::read_to_string(dir.join("libprobe.ttl")).unwrap().contains("ui:noUserResize"));
    lv2::write_bundle_for_editor(&Probe, &dir, "libprobe", None).unwrap();
    assert!(!std::fs::read_to_string(dir.join("manifest.ttl")).unwrap().contains("X11UI"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn descriptor_and_extension_data() {
    let desc = install();
    assert!(!desc.is_null());
    assert!(lv2::lv2ui_descriptor(1).is_null());
    unsafe {
        assert_eq!(CStr::from_ptr((*desc).uri), c"urn:openplugin:test-x11-ui#ui");
        assert!(((*desc).extension_data)(std::ptr::null()).is_null());
    }
    assert!(ext(desc, c"http://lv2plug.in/ns/extensions/ui#showInterface").is_null());

    let idle = ext(desc, c"http://lv2plug.in/ns/extensions/ui#idleInterface") as *const Option<IdleFn>;
    assert!(!idle.is_null());
    let idle = unsafe { (*idle).expect("idle") };
    // No UI behind the handle: the host is told it is gone.
    assert_ne!(unsafe { idle(std::ptr::null_mut()) }, 0);

    let resize = ext(desc, c"http://lv2plug.in/ns/extensions/ui#resize") as *const Resize;
    assert!(!resize.is_null());
    let resize = unsafe { &*resize };
    assert!(resize.handle.is_null());
    assert_ne!(unsafe { (resize.ui_resize.expect("ui_resize"))(std::ptr::null_mut(), 300, 150) }, 0);

    // Calls on no UI are ignored.
    let value = 1.0f32;
    unsafe {
        ((*desc).port_event)(std::ptr::null_mut(), 4, 4, 0, &value as *const f32 as *const c_void);
        ((*desc).cleanup)(std::ptr::null_mut());
    }
}

unsafe extern "C" fn no_write(_: *mut c_void, _: u32, _: u32, _: u32, _: *const c_void) {}

/// Instantiate with these features; returns the handle and the widget.
fn instantiate(desc: *const Lv2UiDescriptor, plugin: &CStr, features: &[(&CStr, *mut c_void)]) -> (*mut c_void, *mut c_void) {
    let owned: Vec<Lv2Feature> = features.iter().map(|(uri, data)| Lv2Feature { uri: uri.as_ptr(), data: *data }).collect();
    let mut list: Vec<*const Lv2Feature> = owned.iter().map(|f| f as *const Lv2Feature).collect();
    list.push(std::ptr::null());
    let mut widget: *mut c_void = std::ptr::null_mut();
    let bundle = CString::new("/nonexistent/").unwrap();
    let handle = unsafe {
        ((*desc).instantiate)(
            desc,
            plugin.as_ptr() as *const c_char,
            bundle.as_ptr(),
            Some(no_write),
            std::ptr::null_mut(),
            &mut widget,
            list.as_ptr(),
        )
    };
    (handle, widget)
}

#[test]
fn opening_the_ui_again_constructs_no_plugin() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let desc = install();
    let plugin = c"urn:openplugin:test-x11-ui";
    let features = [(c"http://lv2plug.in/ns/extensions/ui#parent", 0x7fff_fff0 as *mut c_void)];
    let open = || {
        let made = MADE.load(Ordering::SeqCst);
        let (handle, _) = instantiate(desc, plugin, &features);
        assert_eq!(MADE.load(Ordering::SeqCst), made + 1, "the open got as far as the editor");
        if !handle.is_null() {
            unsafe { ((*desc).cleanup)(handle) };
        }
    };
    // The first open reads what it needs from the plugin once.
    open();
    let built = BUILT.load(Ordering::SeqCst);
    open();
    open();
    assert_eq!(BUILT.load(Ordering::SeqCst), built, "no plugin constructed for a later open");
}

#[test]
fn instantiate_fails_cleanly_without_a_parent_or_a_view() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let desc = install();
    assert!(!desc.is_null());
    let plugin = c"urn:openplugin:test-x11-ui";
    let idle = c"http://lv2plug.in/ns/extensions/ui#idleInterface";
    let parent = c"http://lv2plug.in/ns/extensions/ui#parent";

    // Another plugin's URI, no features, a null feature list, a zero parent:
    // refused before any editor is made.
    let made = MADE.load(Ordering::SeqCst);
    assert!(instantiate(desc, c"urn:someone:else", &[(parent, 0x40_0001 as *mut c_void)]).0.is_null());
    assert!(instantiate(desc, plugin, &[]).0.is_null());
    assert!(instantiate(desc, plugin, &[(idle, std::ptr::null_mut())]).0.is_null());
    assert!(instantiate(desc, plugin, &[(parent, std::ptr::null_mut())]).0.is_null());
    // Wider than an XID.
    assert!(instantiate(desc, plugin, &[(parent, (1usize << 40) as *mut c_void)]).0.is_null());
    let mut widget = std::ptr::null_mut();
    let null_list = unsafe {
        ((*desc).instantiate)(desc, plugin.as_ptr(), c"/".as_ptr(), Some(no_write), std::ptr::null_mut(), &mut widget, std::ptr::null())
    };
    assert!(null_list.is_null() && widget.is_null());
    assert_eq!(MADE.load(Ordering::SeqCst), made, "no editor without a parent window");

    // A parent window that is no real window: with no X server (or today's
    // stub view) the view cannot be made, and with one the attach is refused.
    // Either way the call fails and the editor it made is dropped. Should a
    // server ever accept it, the UI must still clean up after itself.
    let (handle, widget) = instantiate(desc, plugin, &[(idle, std::ptr::null_mut()), (parent, 0x7fff_fff0 as *mut c_void)]);
    assert_eq!(MADE.load(Ordering::SeqCst), made + 1, "one editor made");
    if handle.is_null() {
        assert!(widget.is_null(), "no widget on failure");
    } else {
        assert!(!widget.is_null());
        unsafe { ((*desc).cleanup)(handle) };
    }
    assert_eq!(MADE.load(Ordering::SeqCst), DROPPED.load(Ordering::SeqCst), "every editor dropped");
}
