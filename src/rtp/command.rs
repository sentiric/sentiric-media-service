use std::net::SocketAddr;

#[derive(Debug)]
pub enum RtpCommand {
    PlayAudioUri { // Düzeltme: PlayFile -> PlayAudioUri
        audio_uri: String,
        candidate_target_addr: SocketAddr,
    },
    Shutdown,
}