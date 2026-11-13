//! Which addresses stay in the app's window and which go to the system's browser.

use tauri::{AppHandle, Url};
use tauri_plugin_opener::OpenerExt;

/// True when `url` is a page of the app's own interface, which the core serves at `own`.
pub fn is_own(url: &Url, own: &Url) -> bool {
    url.scheme() == own.scheme()
        && url.host_str() == own.host_str()
        && url.port_or_known_default() == own.port_or_known_default()
}

/// True for the kinds of address that are safe to hand to the system: web pages and mail.
/// Anything else, a `file:` address for one, is neither shown nor handed on.
pub fn may_leave(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "mailto")
}

/// Opens an address with the system's own program for it, the browser for a web page.
pub fn open_outside(app: &AppHandle, url: &Url) {
    if !may_leave(url) {
        tracing::warn!("Not opened, this kind of address is not allowed to leave the app: {url}");
        return;
    }
    if let Err(error) = app.opener().open_url(url.as_str(), None::<&str>) {
        tracing::warn!("Could not open {url} outside the app: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn only_the_cores_own_address_counts_as_the_interface() {
        let own = url("http://127.0.0.1:51234");
        assert!(is_own(&url("http://127.0.0.1:51234/"), &own));
        assert!(is_own(&url("http://127.0.0.1:51234/#/task/7/code"), &own));
        assert!(is_own(&url("http://127.0.0.1:51234/assets/app.js?x=1"), &own));

        // A dev server of the user's project, on another port of the same computer, is not the interface.
        assert!(!is_own(&url("http://127.0.0.1:5173/"), &own));
        assert!(!is_own(&url("http://localhost:51234/"), &own));
        assert!(!is_own(&url("https://127.0.0.1:51234/"), &own));
        assert!(!is_own(&url("https://github.com/owner/repo/pull/7"), &own));
        assert!(!is_own(&url("file:///etc/passwd"), &own));
    }

    #[test]
    fn web_pages_and_mail_may_leave_the_app_and_nothing_else() {
        for allowed in ["https://github.com/owner/repo/pull/7", "http://localhost:5173/", "mailto:someone@example.com"]
        {
            assert!(may_leave(&url(allowed)), "{allowed}");
        }
        for refused in ["file:///etc/passwd", "javascript:alert(1)", "ssh://host/path", "x-apple.systempreferences:x"] {
            assert!(!may_leave(&url(refused)), "{refused}");
        }
    }
}
