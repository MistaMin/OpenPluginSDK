//! The editor in a window of its own, with no host: `window`. The window
//! closes with its close button. Windows and macOS.
use example_editor::make_editor;
use openplugin::core::EditorHost;

fn main() {
    let Some(editor) = make_editor(EditorHost::detached()) else { std::process::exit(1) };
    #[cfg(windows)]
    std::process::exit(openplugin::dri::win32::run_window("Editor Gain", editor, None));
    #[cfg(target_os = "macos")]
    std::process::exit(openplugin::dri::cocoa::run_window("Editor Gain", editor, None));
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        drop(editor);
        eprintln!("window: no editor window on this system yet");
        std::process::exit(2);
    }
}
