//! Only non-exclusive IOHID input callbacks for the five EX-G PRO IDs. No
//! event tap, global keyboard listener, polling loop, HID writes or networking.
use crate::{
    Shared,
    model::{self, Action, SystemAction},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    ffi::{c_char, c_void},
    ptr,
    sync::{Arc, Mutex, OnceLock},
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
    fn CFStringGetCString(string: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
    fn CFPreferencesCopyAppValue(key: Ref, app: Ref) -> Ref;
    fn CFPreferencesAppSynchronize(app: Ref) -> bool;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Ref) -> bool;
    fn CFNumberGetTypeID() -> usize;
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
    fn CGEventSourceCreate(state: i32) -> Ref;
    fn CGEventSourceSetKeyboardType(source: Ref, keyboard_type: u32);
    fn CGEventSourceFlagsState(state: i32) -> u64;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGEventSetType(event: Ref, kind: u32);
    fn CGEventSetFlags(event: Ref, flags: u64);
    fn CGEventPost(location: u32, event: Ref);
}
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub id: String,
    pub family: &'static str,
    pub product_id: u32,
    pub last_extra_button: Option<u8>,
    pub product_name: Option<String>,
    pub transport: Option<String>,
    pub model: &'static str,
    pub buttons: model::ButtonObservation,
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
// CGEvents retain their private source. Pointer ownership is encapsulated;
// access is serialised by KEYBOARD and every event is released exactly once.
struct KeyboardEvent {
    address: usize,
    plan: crate::keyboard::KeyEvent,
    physical: u64,
}
impl Drop for KeyboardEvent {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.address as Ref);
        }
    }
}
static KEYBOARD: OnceLock<Mutex<crate::keyboard::Ownership<KeyboardEvent>>> = OnceLock::new();
static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();
static PICKER: Mutex<(u64, Option<tokio::sync::oneshot::Sender<()>>)> = Mutex::new((0, None));

fn close_picker() {
    if let Ok(mut picker) = PICKER.lock()
        && let Some(cancel) = picker.1.take()
    {
        let _ = cancel.send(());
    }
}

fn show_picker() -> Result<(), String> {
    let mut picker = PICKER.lock().map_err(|_| "Picker lock failed")?;
    if picker.1.is_some() {
        return Ok(());
    }
    let runtime = RUNTIME.get().ok_or("Input runtime is not ready")?;
    let _entered = runtime.enter();
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = executable
        .parent()
        .ok_or("Helper directory is missing")?
        .join("openmouse-app-picker");
    let mut child = tokio::process::Command::new(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("アプリ一覧を開けません。新版の.appから起動してください：{e}"))?;
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    picker.0 = picker.0.wrapping_add(1);
    let generation = picker.0;
    picker.1 = Some(cancel);
    runtime.spawn(async move {
        tokio::select! {
            _ = child.wait() => {},
            _ = cancelled => { let _ = child.kill().await; }
        }
        // A cancelled old process must not clear a newly opened picker.
        if let Ok(mut picker) = PICKER.lock()
            && picker.0 == generation
        {
            picker.1.take();
        }
    });
    Ok(())
}
fn keyboard() -> &'static Mutex<crate::keyboard::Ownership<KeyboardEvent>> {
    KEYBOARD.get_or_init(|| Mutex::new(crate::keyboard::Ownership::default()))
}
fn send_events(events: Vec<KeyboardEvent>) {
    for event in events {
        if event.plan.modifier && !event.plan.down {
            unsafe {
                CGEventSetFlags(
                    event.address as Ref,
                    crate::keyboard::release_flags(
                        event.plan.flags,
                        event.physical,
                        CGEventSourceFlagsState(1),
                    ),
                );
            }
        }
        unsafe {
            CGEventPost(0, event.address as Ref);
        }
        // Before macOS evaluates system shortcuts; no background timer.
        thread::sleep(std::time::Duration::from_millis(
            if event.plan.down && !event.plan.modifier {
                20
            } else {
                1
            },
        ));
    }
}
pub fn cancel_shortcuts() {
    close_picker();
    if let Ok(mut owner) = keyboard().lock() {
        send_events(owner.cancel());
    }
}
fn release_button(device: usize, released: u8) {
    if let Ok(mut owner) = keyboard().lock() {
        send_events(owner.release_button(device, released));
    }
}
fn release_device(device: usize) {
    close_picker();
    if let Ok(mut owner) = keyboard().lock() {
        send_events(owner.release_device(device));
    }
}
fn post_key(key_code: u16, flags: u64) -> Result<(), &'static str> {
    let (press, release) = keyboard_events(key_code, flags)?;
    send_events(press);
    send_events(release);
    Ok(())
}
fn keyboard_events(
    key_code: u16,
    flags: u64,
) -> Result<(Vec<KeyboardEvent>, Vec<KeyboardEvent>), &'static str> {
    unsafe {
        if !AXIsProcessTrusted() {
            return Err("Accessibility permission is required");
        }
        // AX trust alone does not confirm that this process can post events.
        if !CGPreflightPostEventAccess() {
            return Err(
                "macOSがキー送信を許可していません。OpenMouseのアクセシビリティ許可を確認してください。",
            );
        }
        // A private table prevents our events from adopting/mutating the shared
        // hardware state. In particular, never preserve generated low modifier
        // bits when releasing a synthetic Command key.
        let source = CGEventSourceCreate(-1);
        if source.is_null() {
            return Err("Could not create keyboard event source");
        }
        CGEventSourceSetKeyboardType(source, 40);
        let physical = CGEventSourceFlagsState(1);
        let plan = crate::keyboard::stroke(key_code, flags, physical);
        // Allocate *all* presses AND releases before posting the first key.
        let allocate =
            |plan: Vec<crate::keyboard::KeyEvent>| -> Result<Vec<KeyboardEvent>, &'static str> {
                plan.into_iter()
                    .map(|plan| {
                        let event = CGEventCreateKeyboardEvent(source, plan.key, plan.down);
                        if event.is_null() {
                            return Err("Could not create keyboard events");
                        }
                        CGEventSetFlags(event, plan.flags);
                        if plan.modifier {
                            CGEventSetType(event, 12);
                        }
                        Ok(KeyboardEvent {
                            address: event as usize,
                            plan,
                            physical,
                        })
                    })
                    .collect()
            };
        let result = allocate(plan.press)
            .and_then(|press| allocate(plan.release).map(|release| (press, release)));
        CFRelease(source);
        result
    }
}

unsafe fn dictionary_value(dictionary: Ref, name: &'static [u8]) -> Ref {
    unsafe {
        if dictionary.is_null() || CFGetTypeID(dictionary) != CFDictionaryGetTypeID() {
            return ptr::null();
        }
        let key = CFStringCreateWithCString(ptr::null(), name.as_ptr().cast(), 0x08000100);
        let value = CFDictionaryGetValue(dictionary, key);
        CFRelease(key);
        value
    }
}
fn system_key(action: &SystemAction) -> Result<(u16, u64), String> {
    let fallback = action
        .default_key()
        .ok_or("This action does not send keys")?;
    let symbolic_id = action
        .symbolic_id()
        .ok_or("This action has no symbolic hotkey")?;
    unsafe {
        let app = CFStringCreateWithCString(
            ptr::null(),
            c"com.apple.symbolichotkeys".as_ptr(),
            0x08000100,
        );
        let key =
            CFStringCreateWithCString(ptr::null(), c"AppleSymbolicHotKeys".as_ptr(), 0x08000100);
        // Read only when an action is requested; no preference changes or timer.
        CFPreferencesAppSynchronize(app);
        let settings = CFPreferencesCopyAppValue(key, app);
        CFRelease(key);
        CFRelease(app);
        if settings.is_null() {
            return Ok(fallback);
        }
        let resolve = || {
            let entry = dictionary_value(settings, symbolic_id);
            if entry.is_null() {
                return Ok(fallback);
            }
            let enabled = dictionary_value(entry, b"enabled\0");
            if !enabled.is_null()
                && CFGetTypeID(enabled) == CFBooleanGetTypeID()
                && !CFBooleanGetValue(enabled)
            {
                return Err("macOSのキーボード設定で、この操作のショートカットが無効です。".into());
            }
            let value = dictionary_value(entry, b"value\0");
            let params = dictionary_value(value, b"parameters\0");
            if params.is_null()
                || CFGetTypeID(params) != CFArrayGetTypeID()
                || CFArrayGetCount(params) != 3
            {
                return Err("macOS操作のショートカット設定を読み取れません。".into());
            }
            let mut numbers = [0i64; 2];
            for (offset, out) in numbers.iter_mut().enumerate() {
                let number = CFArrayGetValueAtIndex(params, offset as isize + 1);
                if number.is_null()
                    || CFGetTypeID(number) != CFNumberGetTypeID()
                    || !CFNumberGetValue(number, 4, (out as *mut i64).cast())
                {
                    return Err("macOS操作のキー設定が不正です。".into());
                }
            }
            if !(0..=127).contains(&numbers[0]) || numbers[1] < 0 || numbers[1] & !0x00ff0000 != 0 {
                return Err("このmacOS操作のショートカット形式には対応していません。".into());
            }
            Ok((numbers[0] as u16, numbers[1] as u64))
        };
        let result = resolve();
        CFRelease(settings);
        result
    }
}

pub fn perform_action(action: &Action) -> Result<(), String> {
    let mut owner = keyboard().lock().map_err(|_| "Keyboard lock failed")?;
    send_events(owner.take());
    perform_action_unlocked(action)
}
fn perform_button(
    action: &Action,
    device: usize,
    button: u8,
    generation: u64,
) -> Result<(), String> {
    let mut owner = keyboard().lock().map_err(|_| "Keyboard lock failed")?;
    // A save/disable/identify operation cancels any already selected old action.
    if owner.generation != generation {
        return Ok(());
    }
    if !matches!(action, Action::System(binding) if binding.system_action == SystemAction::ApplicationSwitcher)
    {
        close_picker();
    }
    send_events(owner.take());
    if let Action::Shortcut(shortcut) = action
        && crate::keyboard::app_switcher(shortcut.key_code, shortcut.modifiers.flags())
    {
        let (press, release) = keyboard_events(shortcut.key_code, shortcut.modifiers.flags())
            .map_err(str::to_owned)?;
        send_events(press);
        let ticket = owner.hold(crate::keyboard::Held {
            device,
            button,
            release,
        });
        // One bounded safety deadline per hold, not an idle polling loop. A
        // dropped release packet must not leave Command held indefinitely.
        if let Some(runtime) = RUNTIME.get() {
            runtime.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                if let Ok(mut owner) = keyboard().lock() {
                    send_events(owner.expire(ticket));
                }
            });
        }
        return Ok(());
    }
    perform_action_unlocked(action)
}
fn perform_action_unlocked(action: &Action) -> Result<(), String> {
    if !matches!(action, Action::System(binding) if binding.system_action == SystemAction::ApplicationSwitcher)
    {
        close_picker();
    }
    match action {
        Action::Shortcut(shortcut) => {
            post_key(shortcut.key_code, shortcut.modifiers.flags()).map_err(str::to_owned)
        }
        Action::System(binding) => {
            if binding.system_action == SystemAction::ApplicationSwitcher {
                return show_picker();
            }
            // The OS's Mission Control executable calls this exact Dock entry
            // point. Resolve it optionally, so a missing future symbol cannot
            // prevent startup. Only three fixed actions are allowed.
            unsafe {
                let address = dlsym(
                    (-2isize) as *mut c_void,
                    c"CoreDockSendNotification".as_ptr(),
                );
                if !address.is_null() {
                    if !AXIsProcessTrusted() {
                        return Err("Accessibility permission is required".into());
                    }
                    let name = match binding.system_action {
                        SystemAction::MissionControl => c"com.apple.expose.awake",
                        SystemAction::ApplicationWindows => c"com.apple.expose.front.awake",
                        SystemAction::ShowDesktop => c"com.apple.showdesktop.awake",
                        SystemAction::ApplicationSwitcher => unreachable!("handled above"),
                    };
                    let notification =
                        CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x08000100);
                    if notification.is_null() {
                        return Err("Could not create Dock action".into());
                    }
                    let send: unsafe extern "C" fn(Ref, Ref) = std::mem::transmute(address);
                    send(notification, ptr::null());
                    CFRelease(notification);
                    return Ok(());
                }
            }
            let (key, flags) = system_key(&binding.system_action)?;
            post_key(key, flags).map_err(str::to_owned)
        }
    }
}

struct Input {
    device: Device,
    pid: u32,
    buttons: model::ButtonState,
    buttons_initial: bool,
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
unsafe fn string_property(device: Device, name: &'static [u8]) -> Option<String> {
    unsafe {
        let key = CFStringCreateWithCString(ptr::null(), name.as_ptr().cast(), 0x08000100);
        let value = IOHIDDeviceGetProperty(device, key);
        CFRelease(key);
        if value.is_null() {
            return None;
        }
        let mut buffer = [0u8; 256];
        if !CFStringGetCString(
            value,
            buffer.as_mut_ptr().cast(),
            buffer.len() as isize,
            0x08000100,
        ) {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(buffer.as_ptr().cast())
                .to_string_lossy()
                .into_owned(),
        )
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
        || !(1..=64).contains(&length)
    {
        return;
    }
    unsafe {
        let input = &mut *context.cast::<Input>();
        let data = std::slice::from_raw_parts(bytes, length as usize);
        let Some((mask, scope)) = model::button_update(input.pid, report_id, data) else {
            return;
        };
        // Do not turn a button held across startup/reconnection into a shortcut.
        // Movement/scrolling needs no locks or allocations after the baseline.
        let first = input.buttons_initial;
        input.buttons_initial = false;
        let transition = input.buttons.transition_scoped(mask, scope);
        if transition.is_none() && !first {
            return;
        }
        let (previous, mask) = transition.unwrap_or((mask, mask));
        release_button(input.device as usize, previous & !mask);
        let Some((family, _)) = model::product(input.pid) else {
            return;
        };
        let generation = keyboard()
            .lock()
            .map(|owner| owner.generation)
            .unwrap_or(u64::MAX);
        let actions = if input.shared.identifying() || mask & !previous & 0xe0 == 0 {
            Vec::new()
        } else if let Ok(config) = input.shared.config.lock() {
            config.actions(family, previous, mask)
        } else {
            return;
        };
        if let Ok(mut devices) = input.shared.devices.lock()
            && let Some(device) = devices.get_mut(&(input.device as usize))
        {
            device.buttons.update(mask);
            for button in 6..=8 {
                if mask & !previous & (1 << (button - 1)) != 0 {
                    device.last_extra_button = Some(button);
                }
            }
        }
        for (button, action) in actions {
            if let Err(error) = perform_button(&action, input.device as usize, button, generation)
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
            buttons_initial: true,
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
                    product_name: string_property(device, b"Product\0"),
                    transport: string_property(device, b"Transport\0"),
                    model: if family == "mouse" {
                        "M-XG*50MBSK（サイズは本体底面で確認）"
                    } else {
                        "M-XPT1MR"
                    },
                    buttons: model::ButtonObservation::default(),
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
        release_device(device as usize);
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
        let _ = RUNTIME.set(tokio::runtime::Handle::current());
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
