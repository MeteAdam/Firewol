use std::sync::Arc;

use firewol_core::explorer::icons::Icons;
use firewol_core::explorer::shell_menu::{MenuIcon, ShellMenus};
use tauri::{UriSchemeResponder, http::Request};

pub fn respond(
    icons: &Icons,
    thumbnails: &Icons,
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
    let worker = if key.starts_with("thumb:") {
        thumbnails
    } else {
        icons
    };
    let timeout = worker.timeout();
    // A menu's icons are already in memory and last only as long as the menu.
    let (ready, reply) = if key.starts_with("menu:") {
        match menus.icon(&key) {
            Some(MenuIcon::Bitmap(png)) => (Some(png), None),
            Some(MenuIcon::Resource(resource)) => {
                (None, icons.request_menu_resource(resource, size))
            }
            None => (None, None),
        }
    } else {
        (None, worker.request(key, size))
    };
    tauri::async_runtime::spawn_blocking(move || {
        let data: Option<Arc<Vec<u8>>> =
            ready.or_else(|| reply.and_then(|reply| reply.recv_timeout(timeout).ok().flatten()));
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
