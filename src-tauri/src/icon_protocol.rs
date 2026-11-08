use std::sync::Arc;

use firewol_core::explorer::icons::{Icons, TIMEOUT};
use firewol_core::explorer::shell_menu::ShellMenus;
use tauri::{UriSchemeResponder, http::Request};

pub fn respond(
    icons: &Icons,
    menus: &ShellMenus,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let url = tauri::Url::parse(&request.uri().to_string()).ok();
    let mut key = String::new();
    let mut size = 16;
    if let Some(url) = url {
        for (name, value) in url.query_pairs() {
            match name.as_ref() {
                "key" => key = value.into_owned(),
                "s" => size = value.parse().unwrap_or(16),
                _ => {}
            }
        }
    }
    // The file behind a `path:` key can change, so only type icons stay in WebView2's disk cache.
    let lasting = key.starts_with("ext:");
    // A menu's icons are already in memory and last only as long as the menu.
    let ready = key.starts_with("menu:").then(|| menus.icon(&key));
    let reply = if ready.is_none() {
        icons.request(key, size)
    } else {
        None
    };
    tauri::async_runtime::spawn_blocking(move || {
        let data: Option<Arc<Vec<u8>>> = ready
            .flatten()
            .or_else(|| reply.and_then(|reply| reply.recv_timeout(TIMEOUT).ok().flatten()));
        let response = tauri::http::Response::builder()
            .status(if data.is_some() { 200 } else { 404 })
            .header("Content-Type", "image/png")
            .header(
                "Cache-Control",
                if data.is_some() && lasting {
                    "private, max-age=3600"
                } else {
                    "no-store"
                },
            )
            .header("X-Content-Type-Options", "nosniff")
            .body(data.map_or_else(Vec::new, |data| data.as_ref().clone()))
            .expect("icon response");
        responder.respond(response);
    });
}
