#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct ProxyQuery {
    url: String,
    referer: Option<String>,
    headers: Option<String>, // We'll ignore complex header parsing in this minimal version
}

async fn proxy_handler(Query(params): Query<ProxyQuery>) -> impl IntoResponse {
    let client = Client::new();

    let mut request = client.get(&params.url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .header("Accept", "*/*")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Site", "cross-site");

    let referer_url = params
        .referer
        .clone()
        .unwrap_or_else(|| "https://megacloud.blog/".to_string());
    request = request.header("Referer", &referer_url);

    match request.send().await {
        Ok(res) => {
            let mut builder = Response::builder().status(res.status().as_u16());

            // Forward headers
            for (name, value) in res.headers() {
                if name != "transfer-encoding" && name != "content-encoding" {
                    builder = builder.header(name.as_str(), value.as_bytes());
                }
            }

            builder = builder.header("Access-Control-Allow-Origin", "*");

            // For simplicity, we are pulling the full bytes into memory.
            // In a production app you'd want to stream this using axum::body::Body::from_stream
            let body = res.bytes().await.unwrap_or_default();
            builder.body(axum::body::Body::from(body)).unwrap()
        }
        Err(e) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(axum::body::Body::from(format!("Proxy error: {}", e)))
            .unwrap(),
    }
}

async fn run_axum() {
    let app = Router::new().route("/proxy", get(proxy_handler));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .unwrap();
    println!("Background Axum Proxy running on http://127.0.0.1:8080");

    axum::serve(listener, app).await.unwrap();
}

#[tokio::main]
async fn main() {
    tauri::Builder::default()
        .setup(|_app| {
            // Spawn the Axum proxy server in the background
            tokio::spawn(async move {
                run_axum().await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
