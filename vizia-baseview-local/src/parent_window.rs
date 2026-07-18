use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};

pub struct ParentWindow(pub *mut ::std::ffi::c_void);

#[cfg(target_os = "macos")]
impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        use raw_window_handle::AppKitWindowHandle;

        let ns_view = ::std::ptr::NonNull::new(self.0).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(ns_view));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(target_os = "windows")]
impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        use raw_window_handle::Win32WindowHandle;
        use std::num::NonZeroIsize;

        let hwnd = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(target_os = "linux")]
impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        use raw_window_handle::XcbWindowHandle;
        use std::num::NonZeroU32;

        let window_id = NonZeroU32::new(self.0 as u32).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Xcb(XcbWindowHandle::new(window_id));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}
