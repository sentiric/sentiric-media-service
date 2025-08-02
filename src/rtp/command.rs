use std::net::SocketAddr;

#[derive(Debug)]
pub enum RtpCommand {
    PlayFile {
        audio_id: String,
        candidate_target_addr: SocketAddr,
    },
    Shutdown,
}