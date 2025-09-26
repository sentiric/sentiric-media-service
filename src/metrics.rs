// File: src/metrics.rs

use hyper::{service::{make_service_fn, service_fn}, Body, Request, Response, Server as HyperServer, StatusCode};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::convert::Infallible;
use std::net::SocketAddr;
use tracing::{error, info};

pub const GRPC_REQUESTS_TOTAL: &str = "sentiric_media_grpc_requests_total";
pub const ACTIVE_SESSIONS: &str = "sentiric_media_active_sessions";

// YENİ: HTTP isteklerini yönlendirecek fonksiyon
async fn route_handler(req: Request<Body>, recorder_handle: PrometheusHandle) -> Result<Response<Body>, Infallible> {
    match (req.method(), req.uri().path()) {
        // Mevcut metrik yolu
        (&hyper::Method::GET, "/metrics") => {
            let metrics = recorder_handle.render();
            Ok(Response::new(Body::from(metrics)))
        },
        // YENİ sağlık kontrol yolu
        (&hyper::Method::GET, "/healthz") => {
            let response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"status":"ok"}"#))
                .unwrap_or_default();
            Ok(response)
        },
        // Diğer tüm yollar için 404 Not Found
        _ => {
            let mut not_found = Response::default();
            *not_found.status_mut() = StatusCode::NOT_FOUND;
            Ok(not_found)
        }
    }
}

pub fn start_metrics_server(addr: SocketAddr) {
    let recorder_handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("Prometheus recorder kurulumu başarısız oldu");

    tokio::spawn(async move {
        let make_svc = make_service_fn(move |_conn| {
            let recorder_handle = recorder_handle.clone();
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    route_handler(req, recorder_handle.clone())
                }))
            }
        });

        let server = HyperServer::bind(&addr).serve(make_svc);
        
        info!(address = %addr, "Prometheus metrik ve sağlık sunucusu dinlemeye başlıyor...");
        if let Err(e) = server.await {
            error!(error = %e, "Metrik/sağlık sunucusu hatası.");
        }
    });
}