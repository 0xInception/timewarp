//! System-wide bookmark hotkey via Carbon's RegisterEventHotKey: works while
//! League is fullscreen and frontmost, needs no Accessibility or Input
//! Monitoring permission, and never touches the game process.

use std::ffi::c_void;
use std::sync::Mutex;

type OSStatus = i32;
type EventRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type Handler = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: Handler,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OSStatus;
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const SIGNATURE: u32 = u32::from_be_bytes(*b"TWRP");

/// Keys offered in Settings. F1–F5 are League's ally camera keys, so they're out.
pub const KEYS: [&str; 5] = ["F6", "F7", "F8", "F9", "F10"];

fn key_code(name: &str) -> Option<u32> {
    // macOS virtual key codes (kVK_F6...).
    Some(match name {
        "F6" => 0x61,
        "F7" => 0x62,
        "F8" => 0x64,
        "F9" => 0x65,
        "F10" => 0x6D,
        _ => return None,
    })
}

struct State {
    installed: bool,
    hot_key: usize, // EventHotKeyRef as usize so the state is Send
    on_press: Option<Box<dyn Fn() + Send>>,
}

static STATE: Mutex<State> = Mutex::new(State { installed: false, hot_key: 0, on_press: None });

extern "C" fn handler(_: EventHandlerCallRef, _: EventRef, _: *mut c_void) -> OSStatus {
    if let Some(f) = &STATE.lock().unwrap().on_press {
        f();
    }
    0
}

/// (Re)bind the hotkey. Call on the main thread. `None` disables it.
pub fn set(key: Option<&str>, on_press: impl Fn() + Send + 'static) {
    let mut st = STATE.lock().unwrap();
    unsafe {
        if !st.installed {
            let spec = EventTypeSpec { event_class: K_EVENT_CLASS_KEYBOARD, event_kind: K_EVENT_HOT_KEY_PRESSED };
            let mut r: EventHandlerRef = std::ptr::null_mut();
            InstallEventHandler(GetApplicationEventTarget(), handler, 1, &spec, std::ptr::null_mut(), &mut r);
            st.installed = true;
        }
        if st.hot_key != 0 {
            UnregisterEventHotKey(st.hot_key as EventHotKeyRef);
            st.hot_key = 0;
        }
        st.on_press = None;
        let Some(code) = key.and_then(key_code) else { return };
        let mut r: EventHotKeyRef = std::ptr::null_mut();
        let status = RegisterEventHotKey(
            code,
            0,
            EventHotKeyID { signature: SIGNATURE, id: 1 },
            GetApplicationEventTarget(),
            0,
            &mut r,
        );
        if status == 0 {
            log::info!("bookmark hotkey: {}", key.unwrap_or_default());
            st.hot_key = r as usize;
            st.on_press = Some(Box::new(on_press));
        } else {
            log::warn!("could not register hotkey {key:?}: OSStatus {status}");
        }
    }
}
