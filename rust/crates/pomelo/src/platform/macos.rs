use winit::window::{Window, WindowAttributes};

/// Self-managed title bar: content fills under a transparent titlebar, the traffic lights stay top-left and
/// there is no native title strip.
pub(crate) fn window_attributes(attributes: WindowAttributes) -> WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS;
    attributes
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
}

pub(crate) const COMPOSITOR_PACED: bool = false;

pub(crate) fn main_window_attributes(attributes: WindowAttributes) -> WindowAttributes {
    window_attributes(attributes)
}

pub(crate) fn set_dock_icon() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    const ICON: &[u8] = include_bytes!("../../assets/AppIcon.icns");
    unsafe {
        let data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: ICON.as_ptr() as *const std::ffi::c_void,
            length: ICON.len(),
        ];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            eprintln!(
                "[icon] NSImage init from icns FAILED (data_len={})",
                ICON.len()
            );
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        if std::env::var("POMELO_ICON_LOG").is_ok() {
            let valid: bool = msg_send![image, isValid];
            eprintln!("[icon] set applicationIconImage, image_valid={valid} app={app:p}");
        }
    }
}

pub(crate) fn hide_pointer_until_mouse_moves() {
    // SAFETY: a class method taking a plain bool; AppKit shows the pointer again on the next move.
    unsafe { objc2_app_kit::NSCursor::setHiddenUntilMouseMoves(true) };
}

/// Do what the system's "double-click a window's title bar" setting says: fill, zoom, minimize or nothing.
pub(crate) fn titlebar_double_click(window: &Window) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel};
    use objc2_app_kit::NSView;
    use objc2_foundation::NSString;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: the view belongs to a live window of this app; AppKit calls on the main thread.
    unsafe {
        let view: &NSView = &*(handle.ns_view.as_ptr() as *const NSView);
        let Some(ns_window) = view.window() else {
            return;
        };
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let global: *mut AnyObject =
            msg_send![defaults, persistentDomainForName: &*NSString::from_str("NSGlobalDomain")];
        let action: *mut AnyObject = if global.is_null() {
            std::ptr::null_mut()
        } else {
            msg_send![global, objectForKey: &*NSString::from_str("AppleActionOnDoubleClick")]
        };
        let action = if action.is_null() {
            String::new()
        } else {
            (*(action as *const NSString)).to_string()
        };
        match action.as_str() {
            "None" => {}
            "Minimize" => ns_window.miniaturize(None),
            "Fill" => {
                // AppKit's own Fill honours "Tiled windows have margins"; older systems only zoom.
                let fill: bool = msg_send![&*ns_window, respondsToSelector: sel!(_zoomFill:)];
                if fill {
                    let _: () = msg_send![&*ns_window, _zoomFill: std::ptr::null::<AnyObject>()];
                } else {
                    ns_window.zoom(None);
                }
            }
            _ => ns_window.zoom(None),
        }
    }
}

pub(crate) fn pin_layer_top_left(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSString;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    unsafe {
        let view = h.ns_view.as_ptr() as *mut AnyObject;
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return;
        }
        let gravity = NSString::from_str("topLeft");
        let _: () = msg_send![layer, setContentsGravity: &*gravity];
    }
}

pub(crate) fn choose_folders(multiple: bool) -> Vec<std::path::PathBuf> {
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{MainThreadMarker, NSString};
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let panel = unsafe { NSOpenPanel::openPanel(mtm) };
    let (prompt, message) = if multiple {
        ("Add", "Choose the git repositories of the project")
    } else {
        ("Open", "Choose a project folder with a pom.yml")
    };
    unsafe {
        panel.setCanChooseFiles(false);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(multiple);
        panel.setPrompt(Some(&NSString::from_str(prompt)));
        panel.setMessage(Some(&NSString::from_str(message)));
    }
    if unsafe { panel.runModal() } != NSModalResponseOK {
        return Vec::new();
    }
    let urls = unsafe { panel.URLs() };
    urls.iter()
        .filter_map(|url| unsafe { url.path() })
        .map(|path| std::path::PathBuf::from(path.to_string()))
        .collect()
}

/// Native fullscreen (the green button) as AppKit reports it; winit only tracks fullscreen it entered itself.
pub(crate) fn is_native_fullscreen(window: &Window) -> bool {
    use objc2_app_kit::{NSView, NSWindowStyleMask};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return false;
    };
    // SAFETY: the handle's view lives as long as the window, and reading its style mask has no side effects.
    unsafe {
        let view: &NSView = &*(h.ns_view.as_ptr() as *const NSView);
        view.window().is_some_and(|ns_window| {
            ns_window
                .styleMask()
                .contains(NSWindowStyleMask::FullScreen)
        })
    }
}

// Center the macOS traffic lights in our taller top bar: resize the titlebar container
// (two levels up) to a fixed height pinned to the window top, then place the buttons at a constant offset within it,
// so they don't drift when the window is resized.
pub(crate) fn center_traffic_lights(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_app_kit::{NSView, NSWindowButton};
    use objc2_foundation::NSPoint;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    unsafe {
        let view: &NSView = &*(h.ns_view.as_ptr() as *const NSView);
        let Some(ns_window) = view.window() else {
            return;
        };
        // Re-fetch the buttons every time — AppKit can recreate the standard buttons on layout passes.
        let Some(close) = ns_window.standardWindowButton(NSWindowButton::NSWindowCloseButton)
        else {
            return;
        };
        let Some(minimize) =
            ns_window.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton)
        else {
            return;
        };
        let Some(zoom) = ns_window.standardWindowButton(NSWindowButton::NSWindowZoomButton) else {
            return;
        };
        // The titlebar container is TWO levels up (button -> widget container -> titlebar view). Resizing the wrong
        // one (the immediate superview) breaks/hides the buttons.
        let Some(button_container) = close.superview() else {
            return;
        };
        let Some(titlebar) = button_container.superview() else {
            return;
        };

        let catx = AnyClass::get("CATransaction");
        if let Some(catx) = catx {
            let _: () = msg_send![catx, begin];
            let _: () = msg_send![catx, setDisableActions: true];
        }

        let window_h = ns_window.frame().size.height;
        let close_f = close.frame();
        let btn_w = close_f.size.width;
        let btn_h = close_f.size.height;
        let btn_pad = (minimize.frame().origin.x - close_f.origin.x - btn_w).max(0.0);
        let pos_x = 19.0_f64;
        let pos_y = ((workspace::TOP_BAR_H as f64 - btn_h) / 2.0).max(0.0); // vertical padding -> centers in the top bar
        let container_h = btn_h + 2.0 * pos_y;

        // Pin the titlebar container to the top of the window at a fixed height (constant across resizes -> stable).
        let mut tf = titlebar.frame();
        tf.size.height = container_h;
        tf.origin.y = window_h - container_h;
        let _: () = msg_send![&*titlebar, setFrame: tf];

        let min_x = pos_x + btn_w + btn_pad;
        let zoom_x = min_x + btn_w + btn_pad;
        close.setFrameOrigin(NSPoint::new(pos_x, pos_y));
        minimize.setFrameOrigin(NSPoint::new(min_x, pos_y));
        zoom.setFrameOrigin(NSPoint::new(zoom_x, pos_y));
        let _: () = msg_send![&*titlebar, updateTrackingAreas];

        if std::env::var("POMELO_RESIZE_LOG").is_ok() {
            eprintln!(
                "[resize] window_h={window_h:.1} container_h={container_h:.1} titlebar.y={:.1} close=({pos_x:.1},{pos_y:.1}) btn={btn_w:.1}x{btn_h:.1}",
                tf.origin.y
            );
        }

        if let Some(catx) = catx {
            let _: () = msg_send![catx, commit];
        }
    }
}

/// The LaunchAgent that opens the app at login.
fn login_item_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join("Library/LaunchAgents/com.pomelo.app.login.plist"))
}

pub(crate) fn start_at_login() -> bool {
    login_item_path().is_some_and(|path| path.exists())
}

/// Adds or removes the login LaunchAgent for the bundle this run came from.
pub(crate) fn set_start_at_login(enabled: bool) -> std::io::Result<()> {
    let Some(path) = login_item_path() else {
        return Ok(());
    };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe()?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|app| app.extension().is_some_and(|extension| extension == "app"))
        .ok_or_else(|| std::io::Error::other("not running from an app bundle"))?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
  <key>Label</key><string>com.pomelo.app.login</string>\n\
  <key>ProgramArguments</key><array><string>/usr/bin/open</string><string>{}</string></array>\n\
  <key>RunAtLoad</key><true/>\n\
</dict>\n</plist>\n",
        bundle.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, plist)
}

/// The window's own frame resizes it on macOS.
pub(crate) fn resize_edge(
    _window: &Window,
    _cursor: (f64, f64),
) -> Option<winit::window::ResizeDirection> {
    None
}

/// The editing keys are laid out for macOS already: (command, option, control) as pressed.
pub(crate) fn editing_modifiers(
    _key: &winit::keyboard::Key,
    command: bool,
    option: bool,
    control: bool,
    _shift: bool,
) -> (bool, bool, bool) {
    (command, option, control)
}

pub(crate) fn quit_on_termination_signals(_wake: impl Fn() + Send + 'static) {}

pub(crate) fn termination_requested() -> bool {
    false
}
