### **Dosya: `sentiric-media-service/README.md` (Güncel ve Nihai Sürüm)**

# 🎙️ Sentiric Media Service

**Description:** This service is responsible for all real-time media stream (RTP) management within the Sentiric platform. Built with **Rust** for high performance and low-level network control, it handles the allocation of media ports and the future capabilities of processing and manipulating audio/video streams.

**Core Responsibilities:**
*   **Dynamic Port Allocation:** Provides a gRPC endpoint (`AllocatePort`) for other services (like `sip-signaling`) to request and reserve a free UDP port from a predefined range for RTP traffic.
*   **Port Management:** Keeps track of all active and available ports to prevent conflicts and efficiently manage network resources.
*   **RTP Stream Handling:** Listens on allocated ports for incoming RTP traffic. (Future capabilities will include proxying, recording, and analyzing these streams).
*   **Media Playback (Future):** Will expose an API for services like `agent-service` to play audio announcements or hold music into an active RTP stream.

**Technology Stack:**
*   **Language:** Rust
*   **Async Runtime:** Tokio
*   **Inter-Service Communication:**
    *   **gRPC (with Tonic):** Exposes a `MediaService` for synchronous, type-safe control by other backend services.
*   **Containerization:** Docker (Multi-stage builds for minimal, secure images).

**API Interactions (Server For):**
*   **`sentiric-sip-signaling-service` (gRPC):** This service calls the `AllocatePort` RPC to get a media port during the initial SIP call setup.
*   **`sentiric-agent-service` (Future API):** Will call this service to play audio into the call.

## Getting Started

### Prerequisites
- Docker and Docker Compose
- Git
- All Sentiric repositories cloned into a single workspace directory.

### Local Development & Platform Setup
This service is not designed to run standalone. It is an integral part of the Sentiric platform and must be run via the central orchestrator in the `sentiric-infrastructure` repository.

1.  **Clone all repositories:**
    ```bash
    # In your workspace directory
    git clone https://github.com/sentiric/sentiric-infrastructure.git
    git clone https://github.com/sentiric/sentiric-core-interfaces.git
    git clone https://github.com/sentiric/sentiric-media-service.git
    # ... clone other required services
    ```

2.  **Initialize Submodules:** This service depends on `sentiric-core-interfaces` using a Git submodule.
    ```bash
    cd sentiric-media-service
    git submodule update --init --recursive
    cd .. 
    ```

3.  **Configure Environment:**
    ```bash
    cd sentiric-infrastructure
    cp .env.local.example .env
    # Open .env and set RTP_PORT_MIN, RTP_PORT_MAX etc. if needed
    ```

4.  **Run the platform:** The central Docker Compose file will automatically build and run this service.
    ```bash
    # From the sentiric-infrastructure directory
    docker compose up --build -d
    ```

5.  **View Logs:**
    ```bash
    docker compose logs -f media-service
    ```

## Configuration

All configuration is managed via environment variables passed from the `sentiric-infrastructure` repository's `.env` file. See the `.env.local.example` file in that repository for a complete list. Key variables include:
*   `GRPC_HOST`, `GRPC_PORT`: The address this service's gRPC server listens on.
*   `RTP_HOST`: The host IP to bind RTP ports to (`0.0.0.0` for all interfaces).
*   `RTP_PORT_MIN`, `RTP_PORT_MAX`: The UDP port range to be used for media streams.

## Deployment

This service is designed for containerized deployment. The multi-stage `Dockerfile` ensures a small and secure production image. The CI/CD pipeline in `.github/workflows/docker-ci.yml` automatically builds and pushes the image to the GitHub Container Registry (`ghcr.io`).

## Contributing

We welcome contributions! Please refer to the [Sentiric Governance](https://github.com/sentiric/sentiric-governance) repository for detailed coding standards, contribution guidelines, and the overall project vision.

## License

This project is licensed under the [MIT License](LICENSE).
