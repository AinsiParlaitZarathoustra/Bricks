//! The webview's native context menu without « Recharger » in a release build:
//! reloading the page would drop the live state of every conversation. Copy,
//! paste and the other entries stay. A debug build keeps WebKit's menu as is.

#[cfg(all(target_os = "macos", not(debug_assertions)))]
mod imp {
    use objc2::ffi::class_addMethod;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSArray, NSString};

    /// WebKit's identifier of its « Reload » item.
    const RELOAD: &str = "WKMenuItemIdentifierReload";

    type WillOpenMenu = unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, *mut AnyObject);

    /// `-[NSView willOpenMenu:withEvent:]`: WebKit fills the menu, then the
    /// reload item is taken out of it.
    unsafe extern "C-unwind" fn will_open_menu(
        this: &AnyObject,
        cmd: Sel,
        menu: &AnyObject,
        event: *mut AnyObject,
    ) {
        if let Some(method) = this
            .class()
            .superclass()
            .and_then(|parent| parent.instance_method(cmd))
        {
            // SAFETY: the method of the same selector, with the same signature.
            let original: WillOpenMenu = unsafe { std::mem::transmute(method.implementation()) };
            unsafe { original(this, cmd, menu, event) };
        }
        let items: Retained<NSArray<AnyObject>> = unsafe { msg_send![menu, itemArray] };
        for item in items.iter() {
            let id: Option<Retained<NSString>> = unsafe { msg_send![&*item, identifier] };
            if id.is_some_and(|id| id.to_string() == RELOAD) {
                let () = unsafe { msg_send![menu, removeItem: &*item] };
            }
        }
    }

    pub fn install(window: &tauri::WebviewWindow) {
        let _ = window.with_webview(|webview| {
            let view = webview.inner() as *mut AnyObject;
            if view.is_null() {
                return;
            }
            // SAFETY: `inner` is the window's live WKWebView subclass; the
            // method is added once to that class (a second add is refused)
            // with its Objective-C signature: void, self, _cmd, NSMenu, NSEvent.
            unsafe {
                let class: *const AnyClass = (*view).class();
                let imp: Imp = std::mem::transmute(will_open_menu as WillOpenMenu);
                class_addMethod(
                    class as *mut AnyClass,
                    sel!(willOpenMenu:withEvent:),
                    imp,
                    c"v@:@@".as_ptr(),
                );
            }
        });
    }
}

#[cfg(all(target_os = "macos", not(debug_assertions)))]
pub use imp::install;

#[cfg(not(all(target_os = "macos", not(debug_assertions))))]
pub fn install(_window: &tauri::WebviewWindow) {}
