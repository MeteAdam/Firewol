use firewol_core::explorer::icons::{Icons, TIMEOUT};
use tauri::{UriSchemeResponder, http::Request};

pub fn respond(icons: &Icons, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
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
    let reply = icons.request(key, size);
    tauri::async_runtime::spawn_blocking(move || {
        let data = reply.and_then(|reply| reply.recv_timeout(TIMEOUT).ok().flatten());
        let response = tauri::http::Response::builder()
            .status(if data.is_some() { 200 } else { 404 })
            .header("Content-Type", "image/png")
            .header(
                "Cache-Control",
                if data.is_some() {
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
