# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

A self-hosted Stripchat live stream recorder with a web-based management UI. Supports automatic recording, post-processing pipelines, and multi-channel notifications.

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)

---

## Features

- Monitor multiple streamers and auto-record when they go live
- Web UI for managing streamers, recordings, and post-processing
- Supports split network proxies: configure Stripchat API proxy and CDN chunk proxy separately
- Supports configurable Stripchat mirror site (replaces `stripchat.com` in requests with your mirror domain)
- Configurable post-processing pipeline with pluggable modules:
  - **contact_sheet** — generates a tiled preview image with timestamps
  - **filter_short** — deletes recordings below a minimum duration
  - **notify_discord** — sends recording info and cover image to a Discord Webhook
  - **notify_telegram** — sends recording info, cover image, and video via MTProto (supports files >2 GB, HTTP/SOCKS5 proxy)
- Dual runtime: Tauri desktop app or headless server accessible via browser
- Real-time UI updates via Server-Sent Events
- Dark/light mode following system theme
- Custom UI language support, see [Custom Locale Guide](docs/custom-locale.en.md)

---

## Quick Start (Docker)

### docker-compose (recommended)

```yaml
services:
  stripchat-recorder:
    image: chantrail/stripchat-recorder:latest
    container_name: stripchat-recorder
    restart: unless-stopped
    environment:
      - TZ=Asia/Shanghai
    ports:
      - "3030:3030"
    volumes:
      - ./data/logs:/app/stripchat-recorder/logs
      - ./data/recordings:/app/stripchat-recorder/recordings
      - ./data/modules:/app/stripchat-recorder/modules
      - ./data/config:/app/stripchat-recorder/config
```

```bash
docker compose up -d
```

Then open `http://localhost:3030` in your browser.

The Docker image runs in Server mode by default (port 3030). Configuration is written to the mounted `config/settings.json`.

### Network Proxies and Mirror

In the settings page under "Network Proxies", you can configure:

Stripchat mirror project: <https://github.com/ChanTrail/StripchatMirror>

1. API Proxy: used for Stripchat API access; if a mirror is also set, mirror requests go through this proxy.
2. CDN Proxy: used for downloading live stream chunks; can be configured independently from the API proxy.
3. Stripchat Mirror: replaces `stripchat.com` in requests with your mirror domain.

### docker run

```bash
docker run -d \
  --name stripchat-recorder \
  --restart unless-stopped \
  -e TZ=Asia/Shanghai \
  -p 3030:3030 \
  -v ./data/logs:/app/stripchat-recorder/logs \
  -v ./data/recordings:/app/stripchat-recorder/recordings \
  -v ./data/modules:/app/stripchat-recorder/modules \
  -v ./data/config:/app/stripchat-recorder/config \
  chantrail/stripchat-recorder:latest
```

---

## Post-processing Modules

Modules are standalone executables implementing a simple protocol. They receive input via environment variables and communicate with the host via stdout.

### Built-in Modules

| Module            | Description                                                                    |
| ----------------- | ------------------------------------------------------------------------------ |
| `contact_sheet`   | Extracts frames at a configurable interval and tiles them into a preview image |
| `filter_short`    | Deletes recordings shorter than a configurable minimum duration                |
| `notify_discord`  | Sends recording info and cover image to a Discord Webhook                      |
| `notify_telegram` | Sends recording info, cover image, and video to Telegram via MTProto           |

Custom modules placed in the `modules` volume directory are discovered automatically and will not be overwritten when the container restarts. See the [Module Development Guide](docs/module-development.en.md) for details.

---

## Building from Source

**Prerequisites:** Rust, Node.js (LTS), ffmpeg

### First-launch Setup

When running the binary directly, if no run mode has been configured in `config/settings.json`, an interactive TUI will guide you through setup:

1. Select UI language (Chinese / English)
2. Select run mode (Desktop / Server)
3. If Server mode, enter the listen port (default 3030)

The choices are saved to `config/settings.json` and the TUI will not appear again on subsequent launches.

```bash
# Install frontend dependencies
npm install

# Build frontend + Tauri binary
npm run build
npx tauri build --no-bundle

# Build post-processing modules
for dir in modules/*/; do
  [ -f "$dir/Cargo.toml" ] && cargo build --manifest-path "$dir/Cargo.toml" --release --bins
done
```

### Build Docker image

```bash
docker build -t chantrail/stripchat-recorder .
```

---

## Tech Stack

- **Frontend:** Vue 3, TypeScript, Vite, Tailwind CSS, Reka UI
- **Backend / Desktop:** Rust, Tauri 2
- **Post-processing modules:** Rust (standalone binaries)
- **Container:** Debian, ffmpeg

---

## License

This project is licensed under the [GNU General Public License v3.0](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html).

---

## Disclaimer

This project is intended for technical research and learning only. Users are responsible for deployment, operations, and compliance risks.
