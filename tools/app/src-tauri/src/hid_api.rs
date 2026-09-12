// The hidapi context, kept for the life of the HID thread, and shared by every backend that
// talks through hidapi (the gen-1 transport on non-Windows, and the gen-2 transport on all
// platforms). One context per thread, and every device op runs on the one HID thread, so in
// practice that means one context for the process.
//
// HidApi::new() enumerates the whole HID bus, and on macOS that means
// IOHIDManagerSetDeviceMatching on hidapi's process-global manager, which reschedules every
// matched device onto the manager's run loop. Those IOHIDDeviceRefs are cached by IOKit per
// service, so they're the same objects hid_close() has just rescheduled onto the main run
// loop — enumerating on every operation runs that collision over and over, and it is where
// the cable crashed the app on macOS 27 (a pointer-authentication trap on a CFRunLoop inside
// CFRunLoopAddSource).
//
// Nothing here needs a fresh list that often. Device paths stay valid while the cable stays
// plugged in, so hold the context and re-enumerate only when opening from the list we have
// fails — which is exactly the case where the cable has been unplugged or re-plugged and the
// paths really have changed.

use std::cell::RefCell;

use hidapi::HidApi;

thread_local! {
    static API: RefCell<Option<HidApi>> = const { RefCell::new(None) };
}

/// Runs `f` against this thread's hidapi context. If `f` fails on a device list we were
/// already holding, the list is refreshed once and `f` retried; a context we have just built
/// carries a fresh list, so a failure there is real and returned as-is.
pub fn with_api<T>(mut f: impl FnMut(&HidApi) -> Result<T, String>) -> Result<T, String> {
    API.with(|cell| {
        let mut slot = cell.borrow_mut();

        let fresh = slot.is_none();
        if fresh {
            let api = HidApi::new().map_err(|e| e.to_string())?;

            // Don't seize the device: the cable's inline controls stay working while we're
            // open, and we only ever touch the vendor reports.
            #[cfg(target_os = "macos")]
            api.set_open_exclusive(false);

            *slot = Some(api);
        }
        let api = slot.as_mut().expect("context was just placed");

        match f(api) {
            Ok(v) => Ok(v),
            Err(e) if fresh => Err(e),
            Err(_) => {
                api.refresh_devices().map_err(|e| e.to_string())?;
                f(api)
            }
        }
    })
}
