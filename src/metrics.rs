// File: src/metrics.rs

use hyper::{Body, Request, Response, Server as HyperServer};
use metrics_exporter_prometheus::{PrometheusBuilder,
    //  PrometheusHandle
    };
use std::convert::Infallible;
use std::net::SocketAddr;
use tracing::{error, info};

pub const GRPC_REQUESTS_TOTAL: &str = "sentiric_media_grpc_requests_total";
pub const ACTIVE_SESSIONS: &str = "sentiric_media_active_sessions";

pub fn start_metrics_server(addr: SocketAddr) {
    let recorder_handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("Prometheus recorder kurulumu başarısız oldu");

    tokio::spawn(async move {
        let server = HyperServer::bind(&addr).serve(hyper::service::make_service_fn(
            move |_conn| {
                let recorder_handle = recorder_handle.clone();
                async move {
                    Ok::<_, Infallible>(hyper::service::service_fn(move |_: Request<Body>| {
                        let metrics = recorder_handle.render();
                        async move { 
                            Ok::<_, Infallible>(Response::new(Body::from(metrics))) 
                        }
                    }))
                }
            },
        ));
        
        info!(address = %addr, "Prometheus metrik sunucusu dinlemeye başlıyor...");
        if let Err(e) = server.await {
            error!(error = %e, "Metrik sunucusu hatası.");
        }
    });
}