//! Links to the player's logs on the site ("View on mythics.gg").
//!
//! The browser only ever opens a page of the site the app is set to (the
//! origin in Settings): the window and the server hand over a path, never a
//! whole address, and anything that would leave that origin is refused here.
//! The window has no opener permission of its own (capabilities/default.json),
//! so this is the only way a link gets out.
//!
//! The paths are the site's log pages (#327), as `docs/specs/logger-api.md`,
//! "Page links", lists them: the server hands them over (`log_url`,
//! `boss_url`, a pull's or key's `url`), and only those four shapes, with
//! numeric ids, are opened.

use crate::settings::valid_origin;
use tauri::Url;

/// The player's own list of logs.
pub const MY_LOGS: &str = "/account/logs/";

/// A log page's path, if it's one: `/logs/{log}/` (the log, with its Raid
/// and Mythic+ sections), `/logs/{log}/bosses/{encounter}/` (a raid boss),
/// `/logs/{log}/pulls/{fight}/` (one pull) or `/logs/{log}/keys/{fight}/`
/// (a key). Anything else is refused.
pub fn log_page(path: &str) -> Option<String> {
    let inner = path.strip_prefix("/logs/")?.strip_suffix('/')?;
    let parts: Vec<&str> = inner.split('/').collect();
    let ok = match parts.as_slice() {
        [log] => is_id(log),
        [log, what, id] => is_id(log) && matches!(*what, "bosses" | "pulls" | "keys") && is_id(id),
        _ => false,
    };
    ok.then(|| path.to_string())
}

fn is_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 19 && s.bytes().all(|b| b.is_ascii_digit())
}

/// A plain path on the site: `/` then letters, digits and `- . _ ~ /`, with
/// no `..` step and no second leading slash (which a browser reads as
/// another host). No query or fragment: the log pages don't take one.
fn is_site_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= 200
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~'))
        && !path.split('/').any(|seg| seg == "." || seg == "..")
}

/// The whole address for a path on the site, or `None` when it wouldn't be
/// a page of `origin` itself.
pub fn page_url(origin: &str, path: &str) -> Option<String> {
    let origin = valid_origin(origin)?;
    if !is_site_path(path) {
        return None;
    }
    let want = Url::parse(&origin).ok()?;
    let url = Url::parse(&format!("{origin}{path}")).ok()?;
    // Belt and braces: the parsed address must still be that origin.
    (url.origin() == want.origin() && url.path() == path).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://mythics.gg";

    #[test]
    fn a_log_a_raid_boss_a_pull_a_key_and_my_logs() {
        for (path, url) in [
            ("/logs/12/", "https://mythics.gg/logs/12/"),
            (
                "/logs/12/bosses/3129/",
                "https://mythics.gg/logs/12/bosses/3129/",
            ),
            ("/logs/12/pulls/9/", "https://mythics.gg/logs/12/pulls/9/"),
            ("/logs/12/keys/7/", "https://mythics.gg/logs/12/keys/7/"),
        ] {
            assert_eq!(log_page(path).as_deref(), Some(path));
            assert_eq!(
                page_url(SITE, &log_page(path).unwrap()).as_deref(),
                Some(url)
            );
        }
        assert_eq!(
            page_url(SITE, MY_LOGS).as_deref(),
            Some("https://mythics.gg/account/logs/")
        );
    }

    #[test]
    fn only_log_pages_with_numeric_ids() {
        for bad in [
            "",
            "/logs/",
            "/logs/12",
            "/logs/u1/",
            "/logs/12/pulls/",
            "/logs/12/pulls/9",
            "/logs/12/raids/9/",
            "/logs/12/pulls/9/extra/",
            "/logs/12/pulls/../../",
            "/logs/4 1/",
            "/logs/-1/",
            "/logs/12/?x=1",
            "/logs/12345678901234567890/",
            "/account/",
            "https://evil.example/logs/12/",
            "//evil.example/logs/12/",
        ] {
            assert_eq!(log_page(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_local_origin_for_testing() {
        assert_eq!(
            page_url("http://127.0.0.1:4321/", "/logs/41/").as_deref(),
            Some("http://127.0.0.1:4321/logs/41/")
        );
        assert_eq!(
            page_url("http://localhost:8120", MY_LOGS).as_deref(),
            Some("http://localhost:8120/account/logs/")
        );
    }

    #[test]
    fn never_an_address_off_the_site() {
        for path in [
            "",
            "logs/41/",
            "//evil.example/logs/",
            "/\\evil.example/",
            "https://evil.example/",
            "/logs/41/@evil.example",
            "/logs/../../x",
            "/./logs/",
            "/logs/41/?next=https://evil.example",
            "/logs/41/#x",
            "/logs/%2e%2e/",
            "/logs/41/\n",
            "/logs/ 41/",
            "/lögs/",
        ] {
            assert_eq!(page_url(SITE, path), None, "{path:?}");
        }
    }

    #[test]
    fn never_an_origin_settings_would_refuse() {
        for origin in [
            "http://mythics.gg",
            "http://evil.example",
            "javascript:alert(1)",
            "file:///C:/",
            "https://",
            "https://mythics.gg?x=1",
        ] {
            assert_eq!(page_url(origin, "/logs/41/"), None, "{origin:?}");
        }
    }

    #[test]
    fn an_origin_that_hides_another_host_is_refused() {
        // "https://mythics.gg@evil.example" is evil.example to a browser.
        for origin in [
            "https://mythics.gg@evil.example",
            "https://mythics.gg\\@evil.example",
            "http://127.0.0.1@evil.example",
        ] {
            assert_eq!(page_url(origin, "/logs/41/"), None, "{origin:?}");
        }
    }
}
