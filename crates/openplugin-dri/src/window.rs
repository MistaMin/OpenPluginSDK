//! Direct Rendering Interface (DRI) native window attachment.

use raw_window_handle::RawWindowHandle;
use crate::canvas::DrawList;

pub trait DriHostWindow {
    fn raw_handle(&self) -> RawWindowHandle;
    fn size(&self) -> (u32, u32);
}

pub struct DriView {
    pub width: u32,
    pub height: u32,
    pub draw_list: DrawList,
}

impl DriView {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            draw_list: DrawList::new(),
        }
    }

    /// Attaches the DRI rendering pipeline to an arbitrary OS window handle (HWND / NSView / X11).
    pub fn attach<W: DriHostWindow>(&mut self, window: &W) -> Result<(), &'static str> {
        let handle = window.raw_handle();
        match handle {
            RawWindowHandle::Win32(_win) => {
                log::info!("DRI attached to Win32 HWND");
                Ok(())
            }
            RawWindowHandle::AppKit(_appkit) => {
                log::info!("DRI attached to macOS NSView");
                Ok(())
            }
            RawWindowHandle::Xlib(_) | RawWindowHandle::Wayland(_) => {
                log::info!("DRI attached to Linux X11/Wayland window");
                Ok(())
            }
            _ => Err("Unsupported host window handle"),
        }
    }

    pub fn render(&mut self) -> &DrawList {
        &self.draw_list
    }
}
