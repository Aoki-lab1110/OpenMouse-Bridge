//! Only non-exclusive IOHID input callbacks for the five EX-G PRO IDs. No
//! event tap, global keyboard listener, polling loop, HID writes or networking.
use crate::{
    Shared,
    model::{self, Action, Shortcut},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    ffi::{c_char, c_void},
    ptr,
    sync::{Arc, Mutex},
    thread,
};

type Ref = *const c_void;
type Device = *mut c_void;
#[repr(C)]
struct SourceContext {
    version: isize,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(Ref) -> Ref>,
    release: Option<unsafe extern "C" fn(Ref)>,
    description: Option<unsafe extern "C" fn(Ref) -> Ref>,
    equal: Option<unsafe extern "C" fn(Ref, Ref) -> bool>,
    hash: Option<unsafe extern "C" fn(Ref) -> usize>,
    schedule: Option<unsafe extern "C" fn(*mut c_void, Ref, Ref)>,
    cancel: Option<unsafe extern "C" fn(*mut c_void, Ref, Ref)>,
    perform: Option<unsafe extern "C" fn(*mut c_void)>,
}
unsafe extern "C" fn stop_loop(_: *mut c_void) {
    unsafe {
        CFRunLoopStop(CFRunLoopGetCurrent());
    }
}
type DeviceCallback = unsafe extern "C" fn(*mut c_void, i32, *mut c_void, Device);
type ReportCallback = unsafe extern "C" fn(*mut c_void, i32, *mut c_void, u32, u32, *mut u8, isize);
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(alloc: Ref, s: *const c_char, encoding: u32) -> Ref;
    fn CFNumberCreate(alloc: Ref, kind: i32, value: Ref) -> Ref;
    fn CFNumberGetValue(number: Ref, kind: i32, value: *mut c_void) -> bool;
    fn CFDictionaryCreate(
        alloc: Ref,
        keys: *const Ref,
        values: *const Ref,
        count: isize,
        key_callbacks: Ref,
        value_callbacks: Ref,
    ) -> Ref;
    fn CFArrayCreate(alloc: Ref, values: *const Ref, count: isize, callbacks: Ref) -> Ref;
    fn CFRetain(value: Ref) -> Ref;
    fn CFRelease(value: Ref);
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFRunLoopRun();
    fn CFRunLoopSourceCreate(alloc: Ref, order: isize, context: *mut SourceContext) -> Ref;
    fn CFRunLoopAddSource(run_loop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRemoveSource(run_loop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopSourceSignal(source: Ref);
    fn CFRunLoopStop(run_loop: Ref);
    fn CFRunLoopWakeUp(run_loop: Ref);
    static kCFRunLoopDefaultMode: Ref;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFTypeArrayCallBacks: c_void;
    static kCFBooleanTrue: Ref;
}
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDManagerCreate(alloc: Ref, options: u32) -> Device;
    fn IOHIDManagerSetDeviceMatchingMultiple(manager: Device, matches: Ref);
    fn IOHIDManagerRegisterDeviceMatchingCallback(
        manager: Device,
        callback: Option<DeviceCallback>,
        context: *mut c_void,
    );
    fn IOHIDManagerRegisterDeviceRemovalCallback(
        manager: Device,
        callback: Option<DeviceCallback>,
        context: *mut c_void,
    );
    fn IOHIDManagerScheduleWithRunLoop(manager: Device, run_loop: Ref, mode: Ref);
    fn IOHIDManagerUnscheduleFromRunLoop(manager: Device, run_loop: Ref, mode: Ref);
    fn IOHIDManagerOpen(manager: Device, options: u32) -> i32;
    fn IOHIDManagerClose(manager: Device, options: u32) -> i32;
    fn IOHIDDeviceOpen(device: Device, options: u32) -> i32;
    fn IOHIDDeviceClose(device: Device, options: u32) -> i32;
    fn IOHIDDeviceGetProperty(device: Device, key: Ref) -> Ref;
    fn IOHIDDeviceRegisterInputReportCallback(
        device: Device,
        buffer: *mut u8,
        length: isize,
        callback: Option<ReportCallback>,
        context: *mut c_void,
    );
    fn IOHIDDeviceScheduleWithRunLoop(device: Device, run_loop: Ref, mode: Ref);
    fn IOHIDDeviceUnscheduleFromRunLoop(device: Device, run_loop: Ref, mode: Ref);
    fn IOHIDCheckAccess(request_type: u32) -> u32;
    fn IOHIDRequestAccess(request_type: u32) -> bool;
}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: Ref) -> bool;
    static kAXTrustedCheckOptionPrompt: Ref;
    fn CGEventCreateKeyboardEvent(source: Ref, key: u16, down: bool) -> Ref;
    fn CGEventSetFlags(event: Ref, flags: u64);
    fn CGEventPost(location: u32, event: Ref);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub id: String,
    pub family: &'static str,
    pub product_id: u32,
    pub last_extra_button: Option<u8>,
}
pub fn permissions() -> (bool, bool) {
    // kIOHIDRequestTypeListenEvent=1; kIOHIDAccessTypeGranted=0.
    unsafe { (AXIsProcessTrusted(), IOHIDCheckAccess(1) == 0) }
}
pub fn request_permissions() {
    unsafe {
        IOHIDRequestAccess(1);
        let options = CFDictionaryCreate(
            ptr::null(),
            [kAXTrustedCheckOptionPrompt].as_ptr(),
            [kCFBooleanTrue].as_ptr(),
            1,
            ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
            ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
        );
        if !options.is_null() {
            AXIsProcessTrustedWithOptions(options);
            CFRelease(options);
        }
    }
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}
fn post_shortcut(action: &Shortcut) -> Result<(), &'static str> {
    unsafe {
        if !AXIsProcessTrusted() {
            return Err("Accessibility permission is required");
        }
        let down = CGEventCreateKeyboardEvent(ptr::null(), action.key_code, true);
        let up = CGEventCreateKeyboardEvent(ptr::null(), action.key_code, false);
        if down.is_null() || up.is_null() {
            if !down.is_null() {
                CFRelease(down);
            }
            if !up.is_null() {
                CFRelease(up);
            }
            return Err("Could not create keyboard events");
        }
        CGEventSetFlags(down, action.modifiers.flags());
        CGEventSetFlags(up, action.modifiers.flags());
        CGEventPost(0, down);
        CGEventPost(0, up);
        CFRelease(down);
        CFRelease(up);
        Ok(())
    }
}

fn perform_action(action: &Action) -> Result<(), String> {
    match action {
        Action::Shortcut(shortcut) => post_shortcut(shortcut).map_err(str::to_owned),
        Action::System(binding) => {
            // This tiny Apple-supplied program posts a Dock request and exits.
            // No shell, private framework linkage, keyboard remap dependency,
            // background child, network access or arbitrary command execution.
            let result = std::process::Command::new(
                "/System/Applications/Mission Control.app/Contents/MacOS/Mission Control",
            )
            .arg(binding.system_action.argument())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|error| format!("Could not launch macOS Mission Control: {error}"))?;
            if result.success() {
                Ok(())
            } else {
                Err(format!("macOS Mission Control failed: {result}"))
            }
        }
    }
}

struct Input {
    device: Device,
    pid: u32,
    buttons: model::ButtonState,
    buffer: [u8; 64],
    shared: Arc<Shared>,
}
struct Context {
    inputs: HashMap<usize, Box<Input>>,
    shared: Arc<Shared>,
    run_loop: Ref,
}
unsafe fn number_property(device: Device, name: &'static [u8]) -> Option<u32> {
    unsafe {
        let key = CFStringCreateWithCString(ptr::null(), name.as_ptr().cast(), 0x08000100);
        let value = IOHIDDeviceGetProperty(device, key);
        CFRelease(key);
        if value.is_null() {
            return None;
        }
        let mut out = 0i32;
        CFNumberGetValue(value, 3, (&mut out as *mut i32).cast()).then_some(out as u32)
    }
}
unsafe extern "C" fn report(
    context: *mut c_void,
    result: i32,
    _: *mut c_void,
    report_type: u32,
    report_id: u32,
    bytes: *mut u8,
    length: isize,
) {
    if context.is_null()
        || bytes.is_null()
        || result != 0
        || report_type != 0
        || !(7..=8).contains(&length)
    {
        return;
    }
    unsafe {
        let input = &mut *context.cast::<Input>();
        let data = std::slice::from_raw_parts(bytes, length as usize);
        let Some(mask) = model::button_mask(input.pid, report_id, data) else {
            return;
        };
        // Do not turn a button held across startup/reconnection into a shortcut.
        // Movement, scrolling and ordinary clicks need no locks or allocations.
        let Some((previous, mask)) = input.buttons.transition(mask) else {
            return;
        };
        let Some((family, _)) = model::product(input.pid) else {
            return;
        };
        let actions = if let Ok(config) = input.shared.config.lock() {
            config.actions(family, previous, mask)
        } else {
            return;
        };
        if let Ok(mut devices) = input.shared.devices.lock()
            && let Some(device) = devices.get_mut(&(input.device as usize))
        {
            for button in 6..=8 {
                if mask & !previous & (1 << (button - 1)) != 0 {
                    device.last_extra_button = Some(button);
                }
            }
        }
        for action in actions {
            if let Err(error) = perform_action(&action)
                && let Ok(mut value) = input.shared.error.lock()
            {
                *value = Some(error);
            }
        }
    }
}
unsafe extern "C" fn added(context: *mut c_void, result: i32, _: *mut c_void, device: Device) {
    if context.is_null() || device.is_null() || result != 0 {
        return;
    }
    unsafe {
        let context = &mut *context.cast::<Context>();
        if context.inputs.contains_key(&(device as usize)) {
            return;
        }
        if number_property(device, b"VendorID\0") != Some(0x056e) {
            return;
        }
        if number_property(device, b"PrimaryUsagePage\0") != Some(1)
            || number_property(device, b"PrimaryUsage\0") != Some(2)
        {
            return;
        }
        let Some(pid) = number_property(device, b"ProductID\0") else {
            return;
        };
        let Some((family, _)) = model::product(pid) else {
            return;
        };
        // Zero is kIOHIDOptionsTypeNone: never seize the ordinary mouse.
        let result = IOHIDDeviceOpen(device, 0);
        if result != 0 {
            if let Ok(mut error) = context.shared.error.lock() {
                *error = Some(format!("EX-G PRO input could not be opened ({result:#x})"));
            }
            return;
        }
        CFRetain(device);
        let mut input = Box::new(Input {
            device,
            pid,
            buttons: model::ButtonState::default(),
            buffer: [0; 64],
            shared: context.shared.clone(),
        });
        let input_ptr = (&mut *input as *mut Input).cast();
        IOHIDDeviceRegisterInputReportCallback(
            device,
            input.buffer.as_mut_ptr(),
            64,
            Some(report),
            input_ptr,
        );
        IOHIDDeviceScheduleWithRunLoop(device, context.run_loop, kCFRunLoopDefaultMode);
        context.inputs.insert(device as usize, input);
        if let Ok(mut devices) = context.shared.devices.lock() {
            devices.insert(
                device as usize,
                DeviceStatus {
                    id: format!("{family}:{:x}", device as usize),
                    family,
                    product_id: pid,
                    last_extra_button: None,
                },
            );
        }
    }
}
unsafe extern "C" fn removed(context: *mut c_void, _: i32, _: *mut c_void, device: Device) {
    if context.is_null() || device.is_null() {
        return;
    }
    unsafe {
        let context = &mut *context.cast::<Context>();
        if let Some(mut input) = context.inputs.remove(&(device as usize)) {
            IOHIDDeviceUnscheduleFromRunLoop(device, context.run_loop, kCFRunLoopDefaultMode);
            IOHIDDeviceRegisterInputReportCallback(
                device,
                input.buffer.as_mut_ptr(),
                64,
                None,
                ptr::null_mut(),
            );
            IOHIDDeviceClose(device, 0);
            CFRelease(device);
        }
        if let Ok(mut devices) = context.shared.devices.lock() {
            devices.remove(&(device as usize));
        }
    }
}

pub struct Listener {
    run_loop: Arc<Mutex<Option<(usize, usize)>>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Listener {
    pub fn start(shared: Arc<Shared>) -> Self {
        let run_loop = Arc::new(Mutex::new(None));
        let loop_ref = run_loop.clone();
        let thread = thread::spawn(move || unsafe {
            // Do not trigger an OS permission prompt just by starting the app.
            if !permissions().1 {
                if let Ok(mut error) = shared.error.lock() {
                    *error = Some(
                        "Input Monitoring permission is required; grant it, then restart the app"
                            .into(),
                    );
                }
                return;
            }
            let current = CFRunLoopGetCurrent();
            CFRetain(current);
            let manager = IOHIDManagerCreate(ptr::null(), 0);
            if manager.is_null() {
                if let Ok(mut error) = shared.error.lock() {
                    *error = Some("Could not create HID manager".into());
                }
                CFRelease(current);
                return;
            }
            let vendor_key =
                CFStringCreateWithCString(ptr::null(), c"VendorID".as_ptr(), 0x08000100);
            let product_key =
                CFStringCreateWithCString(ptr::null(), c"ProductID".as_ptr(), 0x08000100);
            let page_key =
                CFStringCreateWithCString(ptr::null(), c"PrimaryUsagePage".as_ptr(), 0x08000100);
            let usage_key =
                CFStringCreateWithCString(ptr::null(), c"PrimaryUsage".as_ptr(), 0x08000100);
            let page = 1i32;
            let usage = 2i32;
            let page_value = CFNumberCreate(ptr::null(), 3, (&page as *const i32).cast());
            let usage_value = CFNumberCreate(ptr::null(), 3, (&usage as *const i32).cast());
            let vendor = 0x056ei32;
            let vendor_value = CFNumberCreate(ptr::null(), 3, (&vendor as *const i32).cast());
            let mut dictionaries = Vec::new();
            for pid in [0x013di32, 0x013e, 0x0128, 0x0129, 0x0130] {
                let product_value = CFNumberCreate(ptr::null(), 3, (&pid as *const i32).cast());
                dictionaries.push(CFDictionaryCreate(
                    ptr::null(),
                    [vendor_key, product_key, page_key, usage_key].as_ptr(),
                    [vendor_value, product_value, page_value, usage_value].as_ptr(),
                    4,
                    ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
                    ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
                ));
                CFRelease(product_value);
            }
            let matches = CFArrayCreate(
                ptr::null(),
                dictionaries.as_ptr(),
                5,
                ptr::addr_of!(kCFTypeArrayCallBacks),
            );
            IOHIDManagerSetDeviceMatchingMultiple(manager, matches);
            CFRelease(matches);
            for dictionary in dictionaries {
                CFRelease(dictionary);
            }
            CFRelease(vendor_key);
            CFRelease(product_key);
            CFRelease(vendor_value);
            CFRelease(page_key);
            CFRelease(usage_key);
            CFRelease(page_value);
            CFRelease(usage_value);
            let mut context = Box::new(Context {
                inputs: HashMap::new(),
                shared,
                run_loop: current,
            });
            let context_ptr = (&mut *context as *mut Context).cast();
            IOHIDManagerRegisterDeviceMatchingCallback(manager, Some(added), context_ptr);
            IOHIDManagerRegisterDeviceRemovalCallback(manager, Some(removed), context_ptr);
            IOHIDManagerScheduleWithRunLoop(manager, current, kCFRunLoopDefaultMode);
            let result = IOHIDManagerOpen(manager, 0);
            if result != 0
                && let Ok(mut error) = context.shared.error.lock()
            {
                *error = Some(format!(
                    "Input Monitoring permission may be required ({result:#x}); restart after granting it"
                ));
            }
            // A pending source also works when shutdown arrives before Run().
            let mut source_context = SourceContext {
                version: 0,
                info: ptr::null_mut(),
                retain: None,
                release: None,
                description: None,
                equal: None,
                hash: None,
                schedule: None,
                cancel: None,
                perform: Some(stop_loop),
            };
            let source = CFRunLoopSourceCreate(ptr::null(), 0, &mut source_context);
            if !source.is_null() {
                CFRunLoopAddSource(current, source, kCFRunLoopDefaultMode);
                *loop_ref.lock().unwrap() = Some((current as usize, source as usize));
                CFRunLoopRun();
                // Keep CF objects alive until a concurrent shutdown signal finishes.
                *loop_ref.lock().unwrap() = None;
                CFRunLoopRemoveSource(current, source, kCFRunLoopDefaultMode);
                CFRelease(source);
            } else if let Ok(mut error) = context.shared.error.lock() {
                *error = Some("Could not create native run-loop source".into());
            }
            IOHIDManagerRegisterDeviceMatchingCallback(manager, None, ptr::null_mut());
            IOHIDManagerRegisterDeviceRemovalCallback(manager, None, ptr::null_mut());
            let devices: Vec<_> = context.inputs.keys().copied().collect();
            for device in devices {
                removed(context_ptr, 0, ptr::null_mut(), device as Device);
            }
            IOHIDManagerUnscheduleFromRunLoop(manager, current, kCFRunLoopDefaultMode);
            IOHIDManagerClose(manager, 0);
            CFRelease(manager);
            CFRelease(current);
        });
        Self {
            run_loop,
            thread: Some(thread),
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        loop {
            if let Ok(handle) = self.run_loop.lock()
                && let Some((run_loop, source)) = *handle
            {
                unsafe {
                    CFRunLoopSourceSignal(source as Ref);
                    CFRunLoopWakeUp(run_loop as Ref);
                }
                break;
            }
            if self.thread.as_ref().is_none_or(|t| t.is_finished()) {
                break;
            }
            thread::yield_now();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
