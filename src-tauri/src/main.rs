#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use axum::{
    extract::{Query, State},
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Manager, State as TauriState, WindowEvent};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tower_http::cors::{Any, CorsLayer};

struct ProxyState {
    active: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct ProxyQuery {
    url: String,
    referer: Option<String>,
    headers: Option<String>,
}

#[tauri::command]
fn toggle_proxy(state: TauriState<'_, ProxyState>) -> bool {
    let current = state.active.load(Ordering::Relaxed);
    let new_state = !current;
    state.active.store(new_state, Ordering::Relaxed);
    new_state
}

#[tauri::command]
fn get_proxy_state(state: TauriState<'_, ProxyState>) -> bool {
    state.active.load(Ordering::Relaxed)
}

async fn proxy_handler(
    State(active): State<Arc<AtomicBool>>,
    Query(params): Query<ProxyQuery>,
) -> impl IntoResponse {
    if !active.load(Ordering::Relaxed) {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Access-Control-Allow-Origin", "*")
            .body(axum::body::Body::from("Proxy is currently turned off."))
            .unwrap();
    }

    let client = Client::new();

    // Parse provided headers
    let mut parsed_headers: HashMap<String, String> = HashMap::new();
    if let Some(headers_str) = &params.headers {
        if let Ok(h) = serde_json::from_str::<HashMap<String, String>>(headers_str) {
            parsed_headers = h;
        } else {
            eprintln!("Proxy headers parse error");
        }
    }

    if let Some(referer) = &params.referer {
        if !parsed_headers.contains_key("Referer") && !parsed_headers.contains_key("referer") {
            parsed_headers.insert("Referer".to_string(), referer.clone());
        }
    }

    // Determine Referer and Origin
    let referer_url = parsed_headers
        .get("Referer")
        .or_else(|| parsed_headers.get("referer"))
        .cloned()
        .unwrap_or_else(|| "https://megacloud.blog/".to_string());

    let target_origin = url::Url::parse(&referer_url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| "https://megacloud.blog".to_string());

    // Build Reqwest Headers
    let mut fetch_headers = reqwest::header::HeaderMap::new();

    // Default headers from original Node.js implementation
    let default_headers = vec![
        ("Referer", referer_url.as_str()),
        ("Origin", target_origin.as_str()),
        ("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36"),
        ("Accept", "*/*"),
        ("Accept-Language", "en-US,en;q=0.9"),
        ("Cache-Control", "no-cache"),
        ("Pragma", "no-cache"),
        ("Sec-Ch-Ua", "\"Chrome\";v=\"122\", \"Not(A:Brand\";v=\"24\", \"Google Chrome\";v=\"122\""),
        ("Sec-Ch-Ua-Mobile", "?0"),
        ("Sec-Ch-Ua-Platform", "\"Windows\""),
        ("Sec-Fetch-Dest", "empty"),
        ("Sec-Fetch-Mode", "cors"),
        ("Sec-Fetch-Site", "cross-site"),
        ("Upgrade-Insecure-Requests", "1"),
    ];

    for (k, v) in default_headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            fetch_headers.insert(name, value);
        }
    }

    // Apply custom headers override
    for (k, v) in parsed_headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(&v),
        ) {
            fetch_headers.insert(name, value);
        }
    }

    // Fetch the target URL
    let res = match client.get(&params.url).headers(fetch_headers).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Proxy fetch failed: {}", e);
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(axum::body::Body::from(format!("Proxy Error: {}", e)))
                .unwrap();
        }
    };

    let status = res.status();
    if !status.is_success() {
        eprintln!("Proxy fetch failed for {} (Status: {})", params.url, status);
        return Response::builder()
            .status(status.as_u16())
            .body(axum::body::Body::from(format!(
                "Proxy fetch failed: {}",
                status
            )))
            .unwrap();
    }

    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let is_m3u8 = params.url.contains(".m3u8")
        || content_type.contains("application/vnd.apple.mpegurl")
        || content_type.contains("audio/mpegurl");

    // Rewrite HLS manifest
    if is_m3u8 {
        let text = res.text().await.unwrap_or_default();
        let base_url = if let Some(idx) = params.url.rfind('/') {
            &params.url[..idx + 1]
        } else {
            &params.url
        };

        let mut lines = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                lines.push(line.to_string());
                continue;
            }

            let absolute_url = if trimmed.starts_with("http") {
                trimmed.to_string()
            } else {
                match url::Url::parse(base_url).and_then(|b| b.join(trimmed)) {
                    Ok(u) => u.to_string(),
                    Err(_) => trimmed.to_string(),
                }
            };

            let encoded_url = urlencoding::encode(&absolute_url);
            let mut proxy_url = format!("http://127.0.0.1:4696/proxy?url={}", encoded_url);

            if let Some(h) = &params.headers {
                proxy_url = format!("{}&headers={}", proxy_url, urlencoding::encode(h));
            } else if let Some(r) = &params.referer {
                proxy_url = format!("{}&referer={}", proxy_url, urlencoding::encode(r));
            }

            lines.push(proxy_url);
        }

        let body = lines.join("\n");
        let content_type = if content_type.is_empty() {
            "application/vnd.apple.mpegurl"
        } else {
            &content_type
        };

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "no-cache")
            .body(axum::body::Body::from(body))
            .unwrap()
    } else {
        // Return raw bytes for segments/other files
        let bytes = res.bytes().await.unwrap_or_default();
        let content_type = if content_type.is_empty() {
            "application/octet-stream"
        } else {
            &content_type
        };

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "max-age=3600")
            .body(axum::body::Body::from(bytes))
            .unwrap()
    }
}

async fn run_axum(active: Arc<AtomicBool>) {
    // Implement permissive CORS (Fixes the 403 error for browser preflight OPTIONS requests)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/proxy", get(proxy_handler))
        .layer(cors)
        .with_state(active);

    // Using port 4696 as defined in your setup
    let listener = tokio::net::TcpListener::bind("127.0.0.1:4696")
        .await
        .unwrap();
    println!("Background Axum Proxy running on http://127.0.0.1:4696");

    axum::serve(listener, app).await.unwrap();
}

#[tokio::main]
async fn main() {
    let proxy_active = Arc::new(AtomicBool::new(true));

    let app_proxy_active = proxy_active.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        .manage(ProxyState {
            active: app_proxy_active,
        })
        .invoke_handler(tauri::generate_handler![toggle_proxy, get_proxy_state])
        .setup(|app| {
            // Spawn the Axum proxy server in the background
            tokio::spawn(async move {
                run_axum(proxy_active).await;
            });

            // Tray setup
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>).unwrap();
            let show_i = MenuItem::with_id(app, "show", "Show/Hide", true, None::<&str>).unwrap();
            let menu = Menu::with_items(app, &[&show_i, &quit_i]).unwrap();

            TrayIconBuilder::new()
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let is_visible = window.is_visible().unwrap_or(false);
                            if is_visible {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                    _ => (),
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let is_visible = window.is_visible().unwrap_or(false);
                            if is_visible {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .icon(app.default_window_icon().unwrap().clone())
                .build(app)
                .unwrap();

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                // Hide the window instead of closing
                window.hide().unwrap();
                api.prevent_close();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
