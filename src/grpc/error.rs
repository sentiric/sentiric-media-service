// src/grpc/error.rs
use std::fmt::{Display, Formatter};
use tonic::Status;

#[derive(Debug)]
pub enum ServiceError {
    PortPoolExhausted,
    SessionNotFound { port: u16 },
    InvalidUri { uri: String },
    InvalidTargetAddress { addr: String, source: std::net::AddrParseError },
    CommandSendError(String),
    RecordingSaveFailed { source: String },
    InternalError(anyhow::Error),
}

impl Display for ServiceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::PortPoolExhausted => write!(f, "Available RTP port pool is exhausted."),
            ServiceError::SessionNotFound { port } => write!(f, "Active session not found for port {}.", port),
            ServiceError::InvalidUri { uri } => write!(f, "Unsupported or invalid URI scheme: {}", uri),
            ServiceError::InvalidTargetAddress { addr, .. } => write!(f, "Invalid target RTP address format: {}", addr),
            ServiceError::CommandSendError(msg) => write!(f, "Failed to send command to RTP session: {}", msg),
            ServiceError::RecordingSaveFailed { source } => write!(f, "Failed to finalize and save recording: {}", source),
            ServiceError::InternalError(e) => write!(f, "An internal server error occurred: {}", e),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<ServiceError> for Status {
    fn from(err: ServiceError) -> Self {
        let message = err.to_string();
        match err {
            ServiceError::PortPoolExhausted => Status::resource_exhausted(message),
            ServiceError::SessionNotFound { .. } => Status::not_found(message),
            ServiceError::InvalidUri { .. } | ServiceError::InvalidTargetAddress { .. } => {
                Status::invalid_argument(message)
            }
            
            // --- GÜÇLENDİRİLMİŞ HATA YÖNETİMİ ---
            ServiceError::RecordingSaveFailed { source } => {
                let lower_source = source.to_lowercase();
                if lower_source.contains("nosuchbucket") {
                    let richer_message = format!(
                        "Kayıt hedefi (S3 Bucket) bulunamadı veya yapılandırılmamış. Lütfen altyapıyı kontrol edin. Detay: {}",
                        source
                    );
                    Status::failed_precondition(richer_message)
                } else if lower_source.contains("accessdenied") {
                    let richer_message = format!(
                        "S3 Bucket'ına yazma izni yok (Access Denied). Detay: {}",
                        source
                    );
                    Status::permission_denied(richer_message)
                } else {
                    tracing::error!(error = %message, "Dahili kayıt hatası oluştu");
                    Status::internal("Kayıt kaydedilirken bir iç hata oluştu.")
                }
            }
            // --- HATA YÖNETİMİ SONU ---

            ServiceError::CommandSendError(_) | ServiceError::InternalError(_) => {
                tracing::error!(error = %message, "Dahili servis hatası oluştu");
                Status::internal("Bir iç hata oluştu. Lütfen sunucu loglarını kontrol edin.")
            }              
        }
    }
}