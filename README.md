# Sentiric Media Service

**Description:** Manages real-time media (RTP/SRTP) streams, providing proxy, encryption, and media interaction capabilities for the Sentiric platform.

**Core Responsibilities:**
*   Dynamic allocation and release of RTP/SRTP ports.
*   Proxying RTP/SRTP media streams (audio/video) between participants (acting as a media bridge).
*   SRTP key management, encryption, and decryption of media packets.
*   Playing audio files (announcements, IVR prompts) into RTP streams.
*   Monitoring and publishing media stream quality metrics (packet loss, jitter, etc.).

**Technologies:**
*   Go (or Node.js, depending on chosen implementation)
*   UDP sockets
*   Internal Web/gRPC API for signaling interactions

**API Interactions (As an API Provider):**
*   Exposes APIs for `sentiric-sip-server` to initiate/terminate RTP sessions, start media proxying, and play announcements.
*   Publishes media quality events to `sentiric-cdr-service` (via Message Queue).

**Local Development:**
1.  Clone this repository: `git clone https://github.com/sentiric/sentiric-media-service.git`
2.  Navigate into the directory: `cd sentiric-media-service`
3.  Install dependencies (Go: `go mod tidy` / Node.js: `npm install`).
4.  Create a `.env` file from `.env.example` and configure environment variables (e.g., RTP port range).
5.  Start the service: (Go: `go run main.go` / Node.js: `npm start`).

**Configuration:**
See relevant configuration files and `.env.example` for configurable options. Essential configurations include RTP port ranges and media file paths.

**Deployment:**
This service is designed for containerized deployment (e.g., Docker, Kubernetes). Refer to the `sentiric-infrastructure` repository for deployment configurations, specifically considering `host` networking mode for RTP ports.

**Contributing:**
We welcome contributions! Please refer to the [Sentiric Governance](https://github.com/sentiric/sentiric-governance) repository for coding standards and contribution guidelines.

**License:**
This project is licensed under the [License](LICENSE).
