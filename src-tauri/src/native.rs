//! The native confirmation window: `MessageBoxW` called directly, because the dialog crates do not
//! let the caller pick the default button and Windows picks the first one, which would be the
//! button that allows. `MB_OKCANCEL | MB_DEFBUTTON2` makes Cancel the default, and Escape and the
//! close button also answer Cancel. This is the only unsafe code in the app. It is not covered by
//! unit tests, because it needs a person to look at it; the trait in `confirm.rs` is what tests use.

use tauri::{AppHandle, Manager};
use uuid::Uuid;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST,
    MessageBoxW, PostMessageW, WM_CLOSE,
};

use crate::confirm::{Confirmer, Pending, Prompt};

pub struct NativeConfirmer {
    app: AppHandle,
}

impl NativeConfirmer {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Closes the window if the future that opened it is dropped (cancel, sign-out, timeout).
struct CloseOnDrop {
    title: Vec<u16>,
    armed: bool,
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: `title` is a NUL-terminated UTF-16 string that lives for the call.
        unsafe {
            let window = FindWindowW(std::ptr::null(), self.title.as_ptr());
            if !window.is_null() {
                PostMessageW(window, WM_CLOSE, 0, 0);
            }
        }
    }
}

impl Confirmer for NativeConfirmer {
    fn confirm(&self, prompt: Prompt) -> Pending<'_> {
        let owner: isize = self
            .app
            .get_webview_window("main")
            .and_then(|window| window.hwnd().ok())
            .map_or(0, |hwnd| hwnd.0 as isize);
        Box::pin(async move {
            // The suffix makes the title unique, so a timeout can find this window and no other.
            let title = format!(
                "{} [{}]",
                prompt.title,
                &Uuid::new_v4().simple().to_string()[..6]
            );
            let (title_w, body_w) = (wide(&title), wide(&prompt.body));
            let mut guard = CloseOnDrop {
                title: title_w.clone(),
                armed: true,
            };
            let answer = tauri::async_runtime::spawn_blocking(move || {
                // SAFETY: both strings are NUL-terminated and outlive the call, which blocks until
                // the window is closed. The owner handle may be 0 (no owner).
                let result = unsafe {
                    MessageBoxW(
                        owner as HWND,
                        body_w.as_ptr(),
                        title_w.as_ptr(),
                        MB_OKCANCEL
                            | MB_ICONWARNING
                            | MB_DEFBUTTON2
                            | MB_SETFOREGROUND
                            | MB_TOPMOST,
                    )
                };
                result == IDOK
            })
            .await
            .unwrap_or(false);
            guard.armed = false;
            answer
        })
    }
}
