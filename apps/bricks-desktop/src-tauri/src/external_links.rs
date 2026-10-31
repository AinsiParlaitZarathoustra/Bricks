//! A link followed inside the app (a diagram's link, in its sandboxed frame)
//! opens in the person's browser or mail client, never in the app. The app's
//! own pages (tauri://, the dev server on localhost) load as usual.

use tauri::Url;
use tauri::plugin::{Builder, TauriPlugin};
use tauri_plugin_opener::OpenerExt;

pub fn init<R: tauri::Runtime>() -> TauriPlugin<R> {
    Builder::new("external-links")
        .on_navigation(|webview, url| {
            if !leaves_app(url) {
                return true;
            }
            if let Err(e) = webview.opener().open_url(url.as_str(), None::<&str>) {
                eprintln!("bricks-desktop: {url} could not be opened: {e}");
            }
            false
        })
        .build()
}

/// A web page off this machine, or a mail address.
fn leaves_app(url: &Url) -> bool {
    match url.scheme() {
        "mailto" => true,
        "http" | "https" => !matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "tauri.localhost")
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(url: &str) -> bool {
        leaves_app(&Url::parse(url).unwrap())
    }

    #[test]
    fn web_pages_and_mail_leave_the_app() {
        assert!(leaves("https://example.com/docs"));
        assert!(leaves("http://example.com"));
        assert!(leaves("mailto:someone@example.com"));
    }

    #[test]
    fn the_app_s_own_pages_stay() {
        assert!(!leaves("tauri://localhost/index.html"));
        assert!(!leaves("http://localhost:1420/"));
        assert!(!leaves("http://tauri.localhost/"));
        assert!(!leaves("about:srcdoc"));
        assert!(!leaves("about:blank"));
        assert!(!leaves("data:text/html,x"));
    }
}
