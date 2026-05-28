#![cfg_attr(target_os = "macos", allow(deprecated, unexpected_cfgs, unsafe_op_in_unsafe_fn))]

use anyhow::Result;
#[cfg(target_os = "macos")]
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};
#[cfg(not(target_os = "macos"))]
use std::path::Path;

#[cfg(target_os = "macos")]
use crate::{
    library::Track,
    process::{self, DaemonCommand},
};
#[cfg(not(target_os = "macos"))]
use crate::process;

#[cfg(target_os = "macos")]
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
#[cfg(target_os = "macos")]
static STATUS_ITEM_PTR: OnceLock<usize> = OnceLock::new();
#[cfg(target_os = "macos")]
static BUTTON_PTR: OnceLock<usize> = OnceLock::new();

#[cfg(target_os = "macos")]
pub fn run(data_dir: &Path) -> Result<()> {
    use cocoa::{
        appkit::{
            NSApplication, NSApplicationActivationPolicyAccessory, NSMenu, NSStatusBar,
            NSVariableStatusItemLength,
        },
        base::{id, nil},
    };
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    let _instance_guard = match acquire_instance_lock(data_dir)? {
        Some(guard) => guard,
        None => return Ok(()),
    };

    let _ = DATA_DIR.set(data_dir.to_path_buf());
    let target_class = ensure_target_class();

    unsafe {
        let app = NSApplication::sharedApplication(nil);
        app.setActivationPolicy_(NSApplicationActivationPolicyAccessory);

        let status_bar = NSStatusBar::systemStatusBar(nil);
        let status_item: id = status_bar.statusItemWithLength_(NSVariableStatusItemLength);
        let button: id = msg_send![status_item, button];
        let _ = STATUS_ITEM_PTR.set(status_item as usize);
        let _ = BUTTON_PTR.set(button as usize);

        let target: id = msg_send![target_class, new];
        let menu = NSMenu::new(nil);
        add_item(menu, "Play/Pause", sel!(onPlayPause:), target);
        add_item(menu, "Next", sel!(onNext:), target);
        add_item(menu, "Previous", sel!(onPrevious:), target);
        add_separator(menu);
        add_item(menu, "Quit", sel!(onQuit:), target);

        let _: () = msg_send![status_item, setMenu: menu];
        let _: () = msg_send![button, setTarget: target];
        let _: () = msg_send![button, setAction: sel!(onStatusClick:)];

        let _: id = msg_send![class!(NSTimer),
            scheduledTimerWithTimeInterval: 0.25f64
            target: target
            selector: sel!(onTick:)
            userInfo: nil
            repeats: true
        ];

        let _: () = msg_send![button, setTitle: ns_string(&read_status_title(data_dir))];
        let app_obj = app as *mut Object;
        let _: () = msg_send![app_obj, run];
    }

    Ok(())
}

#[cfg(target_os = "macos")]
struct InstanceLockGuard {
    path: PathBuf,
}

#[cfg(target_os = "macos")]
impl Drop for InstanceLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(target_os = "macos")]
fn acquire_instance_lock(data_dir: &Path) -> Result<Option<InstanceLockGuard>> {
    let lock_path = data_dir.join("menu-bar.lock");
    match OpenOptions::new().write(true).create_new(true).open(&lock_path) {
        Ok(mut file) => {
            let _ = writeln!(file, "{}", std::process::id());
            Ok(Some(InstanceLockGuard { path: lock_path }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if let Ok(raw) = fs::read_to_string(&lock_path) {
                if let Ok(pid) = raw.trim().parse::<i32>() {
                    if !pid_is_running(pid) {
                        let _ = fs::remove_file(&lock_path);
                        return acquire_instance_lock(data_dir);
                    }
                }
            }
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(target_os = "macos")]
fn pid_is_running(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 || *libc::__error() == libc::EPERM }
}

#[cfg(target_os = "macos")]
unsafe fn add_item(menu: cocoa::base::id, title: &str, action: objc::runtime::Sel, target: cocoa::base::id) {
    use cocoa::{appkit::NSMenuItem, base::nil, foundation::NSString};
    use objc::{msg_send, sel, sel_impl};

    let title = NSString::alloc(nil).init_str(title);
    let key = NSString::alloc(nil).init_str("");
    let item = NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(title, action, key);
    let _: () = msg_send![item, setTarget: target];
    let _: () = msg_send![menu, addItem: item];
}

#[cfg(target_os = "macos")]
unsafe fn add_separator(menu: cocoa::base::id) {
    use cocoa::{appkit::NSMenuItem, base::nil};
    use objc::{msg_send, sel, sel_impl};

    let sep = NSMenuItem::separatorItem(nil);
    let _: () = msg_send![menu, addItem: sep];
}

#[cfg(target_os = "macos")]
fn ensure_target_class() -> &'static objc::runtime::Class {
    use objc::{class, declare::ClassDecl, msg_send, runtime::{Class, Object, Sel}, sel, sel_impl};

    if let Some(cls) = Class::get("OrpheusStatusBarTarget") {
        return cls;
    }

    let superclass = Class::get("NSObject").expect("NSObject class");
    let mut decl = ClassDecl::new("OrpheusStatusBarTarget", superclass).expect("declare class");

    extern "C" fn on_play_pause(_: &Object, _: Sel, _: *mut Object) {
        let _ = send_menu_command(DaemonCommand::TogglePause);
    }
    extern "C" fn on_next(_: &Object, _: Sel, _: *mut Object) {
        let _ = send_menu_command(DaemonCommand::Next);
    }
    extern "C" fn on_previous(_: &Object, _: Sel, _: *mut Object) {
        let _ = send_menu_command(DaemonCommand::Previous);
    }
    extern "C" fn on_quit(_: &Object, _: Sel, _: *mut Object) {
        let _ = send_menu_command(DaemonCommand::Stop);
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, terminate: std::ptr::null::<Object>()];
        }
    }
    extern "C" fn on_status_click(_: &Object, _: Sel, _: *mut Object) {
        unsafe {
            let Some(status_item_ptr) = STATUS_ITEM_PTR.get().copied() else {
                return;
            };
            let status_item = status_item_ptr as *mut Object;
            let Some(menu) = current_menu(status_item) else {
                return;
            };
            let _: () = msg_send![status_item, popUpStatusItemMenu: menu];
        }
    }
    extern "C" fn on_tick(_: &Object, _: Sel, _: *mut Object) {
        update_button_title();
    }

    unsafe {
        decl.add_method(sel!(onPlayPause:), on_play_pause as extern "C" fn(&Object, Sel, *mut Object));
        decl.add_method(sel!(onNext:), on_next as extern "C" fn(&Object, Sel, *mut Object));
        decl.add_method(sel!(onPrevious:), on_previous as extern "C" fn(&Object, Sel, *mut Object));
        decl.add_method(sel!(onQuit:), on_quit as extern "C" fn(&Object, Sel, *mut Object));
        decl.add_method(sel!(onStatusClick:), on_status_click as extern "C" fn(&Object, Sel, *mut Object));
        decl.add_method(sel!(onTick:), on_tick as extern "C" fn(&Object, Sel, *mut Object));
    }

    decl.register()
}

#[cfg(target_os = "macos")]
fn send_menu_command(command: DaemonCommand) -> Result<()> {
    let Some(data_dir) = DATA_DIR.get() else {
        return Ok(());
    };
    process::send_command(data_dir, &command)
}

#[cfg(target_os = "macos")]
fn update_button_title() {
    use objc::{msg_send, runtime::Object, sel, sel_impl};

    let Some(data_dir) = DATA_DIR.get() else {
        return;
    };
    let Some(button_ptr) = BUTTON_PTR.get().copied() else {
        return;
    };

    let title = read_status_title(data_dir);
    unsafe {
        let button = button_ptr as *mut Object;
        let _: () = msg_send![button, setTitle: ns_string(&title)];
    }
}

#[cfg(target_os = "macos")]
unsafe fn current_menu(status_item: *mut objc::runtime::Object) -> Option<*mut objc::runtime::Object> {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    let menu: *mut Object = msg_send![status_item, menu];
    (!menu.is_null()).then_some(menu)
}

#[cfg(target_os = "macos")]
unsafe fn ns_string(value: &str) -> cocoa::base::id {
    use cocoa::{base::nil, foundation::NSString};
    NSString::alloc(nil).init_str(value)
}

#[cfg(target_os = "macos")]
fn read_status_title(data_dir: &Path) -> String {
    let Ok(Some(snapshot)) = process::read_snapshot(data_dir) else {
        return String::from("Orpheus");
    };

    let playing = snapshot
        .playing
        .or_else(|| snapshot.current.and_then(|idx| snapshot.queue.get(idx).cloned()));

    let Some(path) = playing else {
        return String::from("Orpheus");
    };

    let display = Track::from_path(PathBuf::from(path)).display_name();
    let normalized = track_title_first(&display);
    truncate_title(&normalized, 28)
}

#[cfg(target_os = "macos")]
fn track_title_first(display: &str) -> String {
    if let Some((artist, title)) = display.split_once(" - ") {
        let artist = artist.trim();
        let title = title.trim();
        if !artist.is_empty() && !title.is_empty() {
            return format!("{title} - {artist}");
        }
    }
    display.to_string()
}

#[cfg(target_os = "macos")]
fn truncate_title(title: &str, max_chars: usize) -> String {
    let count = title.chars().count();
    if count <= max_chars {
        return title.to_string();
    }
    title.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "..."
}
