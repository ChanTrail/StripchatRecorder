# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

A self-hosted Stripchat live stream recorder with a web-based management UI. Supports automatic recording, post-processing pipelines, and multi-channel notifications.

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)

---

## Features

- Monitor multiple streamers and auto-record when they go live
- Web UI for managing streamers, recordings, and post-processing
- **Admin authentication**: password-protected management UI; create a password during the first-launch setup wizard; tokens are bound to login IP, valid for 8 hours, up to 5 concurrent sessions
- **Streamer Finder**: discover streamers via [camgirlfinder.net](https://camgirlfinder.net), supporting:
  - Face search: upload an image, auto-detect a face, and find similar streamers
  - Name search: search by username keyword
  - One-click add to recording list directly from result cards
  - Launch a similar-face search from any streamer card
- **HLS Relay**: proxy a streamer's live stream to any player without recording — open `/stream/{modelname}` to start automatically; supports multiple simultaneous clients
- Supports split network proxies: configure Stripchat API proxy and CDN chunk proxy separately
- Supports configurable Stripchat mirror site (replaces `stripchat.com` in requests with your mirror domain)
- **Mouflon HLS decryption**: manage `pkey → pdkey` key pairs for decrypting Stripchat's encrypted HLS segment filenames; supports automatic key sync from a configured URL
- **Visual post-processing pipeline**: DAG node editor with fork and merge support; built-in modules:
  - **ts_merge** — merge TS segment directories into a single video file (first node of an official pipeline)
  - **contact_sheet** — generate a tiled preview image with timestamps
  - **filter_short** — delete recordings below a minimum duration
  - **notify_discord** — send recording info and cover image to a Discord Webhook
  - **notify_telegram** — send recording info, cover image, and video via MTProto (supports files >2 GB, HTTP/SOCKS5 proxy)
  - **cleanup** — clean up recording-related files and temporary caches after post-processing
- **Community module marketplace**: browse, install, and update community-contributed post-processing modules directly from the Web UI; supports download proxy and mirror configuration
- **About page / auto-update**: check for new releases on GitHub; non-Docker builds support one-click download and automatic binary replacement; Docker builds prompt for manual image update
- Disk space monitoring on the recordings page, with a warning highlight when less than 5 GB remains
- Dual runtime: Tauri desktop app or headless server accessible via browser
- Real-time UI updates via Server-Sent Events with multi-client sync
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
      # - LANGUAGE=zh-CN  # Set to en-US or zh-CN to override interface language
      # - PORT=3030        # Set to override the server port (default: 3030)
    ports:
      - "${PORT:-3030}:${PORT:-3030}"
    volumes:
      - ./data/logs:/app/stripchat-recorder/logs
      - ./data/ts_fragment:/app/stripchat-recorder/ts_fragment
      - ./data/recordings:/app/stripchat-recorder/recordings
      - ./data/modules:/app/stripchat-recorder/modules
      - ./data/meta:/app/stripchat-recorder/meta
      - ./data/config:/app/stripchat-recorder/config
```

```bash
docker compose up -d
```

Then open `http://localhost:3030` in your browser. On first visit, the setup wizard will guide you through language selection, output directory, network proxy, and admin password setup.

The Docker image runs in Server mode by default (port 3030). Configuration is written to the mounted `config/settings.json`.

### docker run

```bash
docker run -d \
  --name stripchat-recorder \
  --restart unless-stopped \
  -e TZ=Asia/Shanghai \
  -e LANGUAGE=en-US \
  -e PORT=3030 \
  -p 3030:3030 \
  -v ./data/logs:/app/stripchat-recorder/logs \
  -v ./data/ts_fragment:/app/stripchat-recorder/ts_fragment \
  -v ./data/recordings:/app/stripchat-recorder/recordings \
  -v ./data/modules:/app/stripchat-recorder/modules \
  -v ./data/meta:/app/stripchat-recorder/meta \
  -v ./data/config:/app/stripchat-recorder/config \
  chantrail/stripchat-recorder:latest
```

---

## Key Settings

The following options are available in the Web UI under Settings:

| Setting                        | Description                                                                             |
| ------------------------------ | --------------------------------------------------------------------------------------- |
| TS stream output directory     | Path where TS segment streams from recordings are stored                                |
| Max concurrent recordings      | Maximum number of simultaneous recordings; `0` means unlimited                          |
| Max concurrent post-processing | Number of simultaneous post-processing tasks; `0` = auto (= logical CPU count); manually set values are capped at logical CPU count × 2 |
| Poll interval (seconds)        | How often to check if a streamer is live; range 10–300                                  |
| Preferred recording resolution | Target recording quality (0 = original/highest); configures fallback direction when unavailable |
| Recording file duration (s)    | Max duration per segment file; `0` = unlimited; useful for splitting long broadcasts    |
| Auto-record on stream start    | Whether newly added streamers have auto-record enabled by default                       |
| Max post-process tmp dir (GB)  | Size limit for temporary files created by post-processing modules; oldest files are deleted when exceeded; `0` = unlimited, default 50 GB |
| Check for beta updates         | Whether to also check pre-release (beta/rc) builds; forced on for beta builds           |

### Network Proxies and Mirror

In the settings page under "Network", you can configure:

Stripchat mirror project: <https://github.com/ChanTrail/StripchatMirror>

1. **API Proxy**: used for Stripchat API access; mirror requests also go through this proxy when a mirror is set.
2. **CDN Proxy**: used for downloading live stream chunks; can be configured independently from the API proxy.
3. **Stripchat Mirror**: replaces `stripchat.com` in requests with your mirror domain.
4. **Community module proxy**: used for downloading community module files; can be set independently.
5. **Community module mirror**: prepends a prefix to GitHub URLs (e.g. `https://ghproxy.com`) to accelerate module downloads.

### Admin Authentication

The first-launch setup wizard guides you through creating an admin password (minimum 6 characters, must include letters, digits, and special characters). After that, every access requires login. Tokens are bound to the login IP, valid for 8 hours with automatic renewal on each request, and up to 5 concurrent sessions are supported.

You can change the password in Settings under the "Security" section.

### Mouflon HLS Decryption Keys

Stripchat encrypts HLS segment filenames (the Mouflon system). If recordings fail to download segments, add the corresponding `pkey → pdkey` key pairs in Settings under "Mouflon Decryption Keys". Keys can be obtained from community channels.

You can also configure a "Sync URL" and optional "Sync Token" to automatically pull the latest keys from a specified URL.

### HLS Relay

In Server mode, you can stream any streamer's live broadcast directly to a player without adding them to the recording list:

```
http://localhost:3030/stream/{modelname}
```

The relay starts automatically on first access and supports multiple simultaneous clients on the same stream. The "Relay" page in the Web UI shows all active sessions with their state, connection count, and uptime.

---

## Post-processing Modules

The post-processing pipeline is a **DAG (directed acyclic graph)**. Use the visual editor to build processing chains by dragging and connecting nodes, with support for branching and merging.

### Built-in Modules

| Module            | Description                                                                              |
| ----------------- | ---------------------------------------------------------------------------------------- |
| `ts_merge`        | Merges a TS segment directory into a single video file; **first node in an official pipeline** |
| `contact_sheet`   | Extracts frames at a configurable interval and tiles them into a preview image           |
| `filter_short`    | Deletes recordings shorter than a configurable minimum duration                          |
| `notify_discord`  | Sends recording info and cover image to a Discord Webhook                                |
| `notify_telegram` | Sends recording info, cover image, and video to Telegram via MTProto (supports files >2 GB, HTTP/SOCKS5 proxy) |
| `cleanup`         | Cleans up recording-related files and temporary caches; typically the last pipeline node |

### Community Module Marketplace

The "Module Community" page in the Web UI lets you browse, install, and update third-party post-processing modules contributed by the community. A disclaimer is shown before installation. Download proxy and mirror settings are configurable in Settings.

Custom modules placed in the `modules` volume directory are discovered automatically and will not be overwritten when the container restarts. See the [Module Development Guide](docs/module-development.en.md) and [Community Module Registry](docs/community-registry.en.md) for details.

> **Filename format:** The host only discovers executables whose filename matches `{name}-{platform}-{version}[.exe]`.

### Desktop Module Install Path

The Desktop installer packages (NSIS/MSI, AppImage, deb/rpm, dmg) do not place modules inside the app's install directory. Extract the downloaded `modules-{platform}.zip` into the per-user data directory's `modules` subdirectory:

| Platform | Path                                                                              |
| -------- | --------------------------------------------------------------------------------- |
| Windows  | `%APPDATA%\com.chantrail.stripchat-recorder\modules\`                             |
| macOS    | `~/Library/Application Support/com.chantrail.stripchat-recorder/modules/`         |
| Linux    | `~/.local/share/com.chantrail.stripchat-recorder/modules/` (or `$XDG_DATA_HOME`) |

You can create the directory manually if it doesn't exist yet. The app watches this directory at runtime, so adding or removing modules doesn't require a restart.

---

## Building from Source

**Prerequisites:** Rust, Node.js (LTS), ffmpeg

### First-launch Setup

When running the binary directly, if the setup wizard has not been completed in `config/settings.json`, a web-based setup wizard will appear on first visit:

1. Select UI language
2. Set the recording output directory
3. Configure network proxy (optional)
4. Set the admin password

The choices are saved to `config/settings.json` and the wizard will not appear again on subsequent launches.

```bash
# Install frontend dependencies
npm install

# Build Server frontend + backend
npm run build

# Build Desktop version
npm run build:desktop

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
