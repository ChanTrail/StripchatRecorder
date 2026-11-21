# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

A self-hosted Stripchat live stream recorder with a web-based management UI. Supports automatic recording, post-processing pipelines, and multi-channel notifications.

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)
[![GHCR](https://img.shields.io/badge/ghcr.io-chantrail%2Fstripchat--recorder-blue)](https://github.com/ChanTrail/StripchatRecorder/pkgs/container/stripchat-recorder)

---

## Features

- Monitor multiple streamers and auto-record when they go live
- Web UI for managing streamers, recordings, and post-processing
- **Admin authentication**: password-protected management UI; create a password during the first-launch setup wizard; tokens are bound to login IP, valid for 8 hours, up to 5 concurrent sessions
- **Streamer Finder**: discover streamers via [camgirlfinder.net](https://camgirlfinder.net), supporting:
    - Face search: upload an image or paste an image URL to auto-detect a face and find similar streamers; optional background mode shows the full result images
    - Name search: search by username keyword
    - Cross-forwarding between modes: launch a face search on any streamer from name results, or switch to a name search from face results
    - One-click add to recording list directly from result cards
- **Rename tracking**: at add time, fetches and persists the Stripchat internal user ID via the user-ID API; all subsequent status polls query by ID directly, enabling automatic rename detection and record updates without manual re-adding
- **StripChat browse page**: browse live streams by official Stripchat categories (Girls/Couples/Men/Trans) with trait & language tag filters, sorting, and infinite scroll — add a streamer to the tracked list with one click on the card; card badges flag features like interactive toys (TOY); card status silently refreshes on the poll interval; on the desktop app, clicking a card opens the built-in relay stream in a local player (a custom player or auto-detected mpv/VLC/PotPlayer/IINA etc.); list and snapshot requests use their own dedicated proxy and mirror settings
- **Privacy mode**: blur thumbnails with three levels (Off / Live only / All images); toggled independently on the streamer list page and the StripChat browse page, and any single image can be unblurred individually
- **HLS Relay**: proxy a streamer's live stream to any player without recording — open `/stream/{modelname}` to start automatically; supports multiple simultaneous clients; `?resolution=&codec=&preference=` query params temporarily override the stream selection
- Supports split network proxies: configure the Stripchat API proxy, CDN chunk proxy, browse-page proxy, and camgirlfinder proxy separately
- Supports configurable Stripchat mirror site (replaces `stripchat.com` in requests with your mirror domain, http/https scheme selectable); the browse page has its own independent mirror setting
- **ECH (Encrypted Client Hello)**: optional — encrypts the TLS ClientHello of StripChat API requests with a borrowed ECH config to hide the real SNI, which may mitigate HTTP 403 responses; automatically falls back to a plain connection when the host is unroutable or key resolution fails; CDN segment traffic only rides ECH legs in the fallback race after every direct attempt has failed
- **Mouflon HLS decryption**: manage `pkey → pdkey` key pairs and `pkey → keystream` key streams for decrypting Stripchat's encrypted HLS segment filenames; supports automatic key sync from a configured URL
- **Visual post-processing pipeline**: DAG node editor with fork and merge support; built-in modules:
    - **segment_merge** — merge recorded segment directories (fMP4 .m4s) into a single video file (first node of an official pipeline)
    - **contact_sheet** — generate a tiled preview image with timestamps
    - **filter_short** — delete recordings below a minimum duration
    - **notify_discord** — send recording info and cover image to a Discord Webhook
    - **notify_telegram** — send recording info, cover image, and video via MTProto (supports files >2 GB, HTTP/SOCKS5 proxy)
    - **cleanup** — clean up recording-related files and temporary caches after post-processing
- **Community module marketplace**: browse, install, and update community-contributed post-processing modules directly from the Web UI; supports download proxy and mirror configuration
- **About page / auto-update**: check for new releases on GitHub; non-Docker builds support one-click download and automatic binary replacement; Docker builds prompt for manual image update; release notes are rendered as Markdown and expandable inline, alongside a contributors list and links to the repo / Telegram channel / FFmpeg build source
- **Service status page**: continuously probes connectivity and latency of every dependency (StripChat API, StripChat browse page, camgirlfinder, GitHub, CDN domains) with ECG-style rolling graphs; probes go through the same channels as production (proxy / ECH / mirror per current settings), plain and ECH legs side by side, with provider badges auto-detected from response headers
- **Resource usage page**: per-process-tree stats (main / module / ffmpeg / other, including grandchildren spawned by modules) — CPU, memory, PID, disk read/write rates, and GPU utilization & VRAM (sampled via Windows PDH / Linux fdinfo); plus a system network card and pinned CPU/memory/disk/network/GPU trend charts
- **In-app notification panel**: persisted events such as streamer renames and dead-streamer alerts, FFmpeg install/update milestones, and new-version notices
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
        # image: ghcr.io/chantrail/stripchat-recorder:latest  # GHCR mirror, published in sync with Docker Hub — pick whichever registry your network reaches
        container_name: stripchat-recorder
        restart: unless-stopped
        environment:
            - TZ=Asia/Shanghai
            # - LANGUAGE=zh-CN  # Set to en-US or zh-CN to override interface language
            # - PORT=3030        # Set to override the server port (default: 3030)
        ports:
            - "${PORT:-3030}:${PORT:-3030}"
        volumes:
            - ./data/logs:/app/stripchat-recorder/logs # Runtime logs
            - ./data/fragment:/app/stripchat-recorder/fragment # Recorded segments during recording (segment output dir)
            - ./data/recordings:/app/stripchat-recorder/recordings # Video output from segment_merge and other modules
            - ./data/modules:/app/stripchat-recorder/modules # Post-processing module executables
            - ./data/meta:/app/stripchat-recorder/meta # Recording metadata
            - ./data/config:/app/stripchat-recorder/config # Config files (including settings.json)
            - ./data/tools:/app/stripchat-recorder/tools # FFmpeg (auto-downloaded or bring-your-own)
```

```bash
docker compose up -d
```

Then open `http://localhost:3030` in your browser. On first visit, the setup wizard will guide you through language selection, output directory, network proxy, an FFmpeg source check, and admin password setup.

The Docker image runs in Server mode by default (port 3030). Configuration is written to the mounted `config/settings.json`. The image is built multi-arch (`linux/amd64`, `linux/arm64`), so both x86 and ARM64 hosts can pull and run it directly. Each tag is published to both **Docker Hub** (`chantrail/stripchat-recorder`) and **GHCR** (`ghcr.io/chantrail/stripchat-recorder`) with identical content — use whichever registry your network can reach. **FFmpeg is not baked into the image**: the setup wizard downloads it into the mounted `tools` dir on first start (requires access to GitHub Releases), or drop a self-provided build into `tools` beforehand — see "Hardware requirements & the `-legacy` track" below.

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
  -v ./data/fragment:/app/stripchat-recorder/fragment \
  -v ./data/recordings:/app/stripchat-recorder/recordings \
  -v ./data/modules:/app/stripchat-recorder/modules \
  -v ./data/meta:/app/stripchat-recorder/meta \
  -v ./data/config:/app/stripchat-recorder/config \
  -v ./data/tools:/app/stripchat-recorder/tools \
  chantrail/stripchat-recorder:latest
# For GHCR, replace the image name with ghcr.io/chantrail/stripchat-recorder:latest
```

---

## Key Settings

The following options are available in the Web UI under Settings:

| Setting                        | Description                                                                                                                                                                                                                                                                     |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Segment output directory       | Path where recorded segment streams (fMP4 .m4s) are stored                                                                                                                                                                                                                      |
| Max concurrent post-processing | Number of simultaneous post-processing tasks; `0` = auto (= logical CPU count); manual values are capped at the same logical CPU count — useful for deliberately lowering the ceiling. Actual concurrency is scaled down automatically under system load                        |
| Poll interval (seconds)        | Base interval for checking whether a streamer is live; range 10–300. For streamers with historical online-schedule data, the interval stretches adaptively during low-activity periods (up to 200 s) to save bandwidth                                                          |
| Preferred recording resolution | Target recording quality (0 = original/highest); configures fallback direction when unavailable                                                                                                                                                                                 |
| Preferred video codec          | Sent as the `preferredVideoCodec` parameter when fetching playlists (H.264 / H.265 / AV1, unset = server decides); falls back downward AV1→H.265→H.264 when the chosen codec is unavailable; H.265 / AV1 produces smaller files but requires player and post-processing support |
| Recording file duration (s)    | Max duration per segment file; `0` = unlimited; useful for splitting long broadcasts                                                                                                                                                                                            |
| Max post-process tmp dir (GB)  | Size limit for temporary files created by post-processing modules; oldest files are deleted when exceeded; `0` = unlimited, default 50 GB                                                                                                                                       |
| FFmpeg preference              | Choose between the FFmpeg on the system PATH and the one in the program's `tools` folder; when unset, a selection wizard pops up after launch                                                                                                                                   |
| Auto-update FFmpeg when idle   | Periodically checks and auto-updates the program-downloaded FFmpeg in `tools` while no recording or post-processing task is running; on by default                                                                                                                              |
| Check for beta updates         | Whether to also check pre-release (beta) builds; forced on for beta builds                                                                                                                                                                                                      |
| Custom player (Desktop)        | Player program launched for relay streams, recording files, and media files; empty = auto-detect (relay streams probe mpv/VLC/PotPlayer etc., recordings use the system association)                                                                                              |

### Network Proxies and Mirror

In the settings page under "Network", you can configure:

Stripchat mirror project: <https://github.com/ChanTrail/StripchatMirror>

1. **API Proxy**: used for Stripchat API access; mirror requests also go through this proxy when a mirror is set.
2. **Stripchat Mirror**: replaces `stripchat.com` in requests with your mirror domain; http/https scheme selectable.
3. **Browse-page proxy**: dedicated to StripChat browse page list/snapshot requests, independent from the API proxy; does not affect recording or status polling.
4. **Browse-page mirror**: dedicated mirror for the StripChat browse page, independent from the mirror above; accessed through the browse-page proxy when both are set.
5. **CDN Proxy**: used for downloading live stream chunks; can be configured independently from the API proxy.
6. **Community module proxy**: used for downloading community module files; can be set independently.
7. **Community module mirror**: prepends a prefix to GitHub URLs (e.g. `https://ghproxy.com`) to accelerate module downloads; accessed through the community proxy when both are set.
8. **camgirlfinder proxy**: used by the backend to fetch each streamer's historical online schedule for adaptive poll intervals; the Streamer Finder page's face/name search is a pure-frontend feature and does not use this proxy.
9. **ECH (Encrypted Client Hello)**: when enabled, TLS ClientHellos for StripChat API requests are encrypted to hide the real SNI, which may mitigate HTTP 403 responses; automatically falls back to a plain connection when the host is unroutable or key resolution fails. CDN segment traffic prefers the direct try and rides ECH only in the fallback race after every direct attempt has failed. Off by default.

### Admin Authentication

The first-launch setup wizard guides you through creating an admin password (minimum 6 characters, must include letters, digits, and special characters). After that, every access requires login. Tokens are bound to the login IP, valid for 8 hours with automatic renewal on each request, and up to 5 concurrent sessions are supported.

You can change the password in Settings under the "Security" section.

### Mouflon HLS Decryption Keys

Stripchat encrypts HLS segment filenames (the Mouflon system). If recordings fail to download segments, add the corresponding `pkey → pdkey` key pairs in Settings under "Mouflon Decryption Keys"; when there is no pdkey you can fill in a `keystream` instead (hex, at least 16 bytes), and when a `pkey` has both, the pdkey is used. Keys can be obtained from community channels.

You can also configure a "Sync URL" and optional "Sync Token" to automatically pull the latest keys from a specified URL.

### HLS Relay

In Server mode, you can stream any streamer's live broadcast directly to a player without adding them to the recording list; the relay starts automatically on first access and supports multiple simultaneous clients on the same stream. The "Relay" page in the Web UI shows all active sessions with their state, connection count, and uptime.

```
http://localhost:3030/stream/{modelname}?resolution=720&codec=h264&preference=higher
```

Query params temporarily override the stream selection (missing fields fall back to settings; a request with different params restarts the session so the new params apply):

- `resolution`: target resolution height in px (`0` = source quality)
- `codec`: preferred video codec (`h264` / `h265` / `av1`, case-insensitive)
- `preference`: fallback direction when the target resolution is unavailable (`higher` = prefer upward)

---

## Post-processing Modules

The post-processing pipeline is a **DAG (directed acyclic graph)**. Use the visual editor to build processing chains by dragging and connecting nodes, with support for branching and merging.

### Built-in Modules

| Module            | Description                                                                                                    |
| ----------------- | -------------------------------------------------------------------------------------------------------------- |
| `segment_merge`   | Merges a recorded segment directory into a single video file; **first node in an official pipeline**           |
| `contact_sheet`   | Extracts frames at a configurable interval and tiles them into a preview image                                 |
| `filter_short`    | Deletes recordings shorter than a configurable minimum duration                                                |
| `notify_discord`  | Sends recording info and cover image to a Discord Webhook                                                      |
| `notify_telegram` | Sends recording info, cover image, and video to Telegram via MTProto (supports files >2 GB, HTTP/SOCKS5 proxy) |
| `cleanup`         | Cleans up recording-related files and temporary caches; typically the last pipeline node                       |

### Community Module Marketplace

The "Module Community" page in the Web UI lets you browse, install, and update third-party post-processing modules contributed by the community. A disclaimer is shown before installation. Download proxy and mirror settings are configurable in Settings.

Custom modules placed in the `modules` volume directory are discovered automatically and will not be overwritten when the container restarts. See the [Module Development Guide](docs/module-development.en.md) and [Community Module Registry](docs/community-registry.en.md) for details.

> **Filename format:** The host only discovers executables whose filename matches `{name}-{platform}-{version}[.exe]`.

### Desktop Module Install Path

The Desktop installer packages (NSIS, deb, dmg) do not place modules inside the app's install directory. Extract the downloaded `modules-{platform}.zip` into the per-user data directory's `modules` subdirectory:

| Platform | Path                                                                             |
| -------- | -------------------------------------------------------------------------------- |
| Windows  | `%APPDATA%\com.chantrail.stripchat-recorder\modules\`                            |
| macOS    | `~/Library/Application Support/com.chantrail.stripchat-recorder/modules/`        |
| Linux    | `~/.local/share/com.chantrail.stripchat-recorder/modules/` (or `$XDG_DATA_HOME`) |

You can create the directory manually if it doesn't exist yet. The app watches this directory at runtime, so adding or removing modules doesn't require a restart.

**Portable builds** (Windows `*-portable.zip`, Linux AppImage): the data directory travels with the program — `config/`, `modules/`, `tools/` etc. live next to the executable (next to the exe on Windows, next to the `.AppImage` file on Linux), so modules just go into the adjacent `modules/`. The Windows portable zip already bundles the modules. On first launch, `config/`, `locale/` and `modules/` are migrated from the old per-user data dir of an installed build when the portable-side counterpart doesn't exist yet.

---

## FFmpeg Management

The program depends on ffmpeg and ffprobe, managed automatically — manual installation is usually unnecessary:

- **Lookup order**: on startup the program first checks the `tools` folder (Server: next to the program; Desktop: under the same data root as `modules` — the per-user data dir for installed builds, the program's own directory for portable/AppImage), then the system PATH. The `tools` copy takes priority over PATH.
- **Auto-download**: if neither location has them, a prebuilt static build (~130–210 MB; 7z solid archive on Windows, tar.xz on Linux/macOS) is downloaded from [FFmpeg-Builds-Full](https://github.com/ChanTrail/FFmpeg-Builds-Full) into `tools`, with progress shown as a toast (supported: Windows x86_64/ARM64, Linux x86_64/ARM64, macOS ARM64).
- **FFmpeg preference**: while no source has been chosen (`ffmpeg_preference` unset, e.g. after upgrading from an older version), a selection wizard pops up at launch showing detection results for each location so you can pick the system PATH or the `tools` copy — it can also be changed anytime in Settings; auto-update is disabled while the PATH source is selected.
- **Idle auto-update**: the downloaded copy is kept up to date automatically while no recording or post-processing task is running (Settings → "Auto-update FFmpeg when idle", on by default; a check also runs at program start). An ffmpeg from PATH, or one placed in `tools` manually without the `ffmpeg-build.json` marker, is never replaced. Milestones (new version detected → install started → done/failed) are recorded to the in-app notification panel.
- The **Docker image** does not bundle FFmpeg: on first start the setup wizard guides you to download it in place into `tools` (a `VOLUME` in the image, so the downloaded copy persists with its `ffmpeg-build.json` marker and participates in idle auto-update inside the container too) — the track and variant are selectable in the wizard's download step; you can also pick a system-PATH ffmpeg, or drop a self-provided build into the mounted `tools` dir beforehand, which the program then prefers.

### Hardware requirements & the `-legacy` track

FFmpeg-Builds-Full publishes **platform tracks** (linux ships a default track and a `-legacy` compatible track) and **variant shapes** (static / `-shared` / `-debug` / `-shared-debug`). Both tracks enable the same libraries and features — they differ only in CPU baseline generation:

| Track                                               | x86_64                                                                                           | arm64                                   |
| --------------------------------------------------- | ------------------------------------------------------------------------------------------------ | --------------------------------------- |
| Default (`linux-x86_64` / `linux-arm64`)            | **x86-64-v3+** (AVX2/BMI2/FMA3, ≈Haswell/2013 onward)                                            | Any ARMv8                               |
| `-legacy` (`linux-x86_64-legacy` / `-arm64-legacy`) | SSE2 baseline — any x86-64 CPU (AVX2+ SIMD uses cpuid runtime dispatch, old CPUs fall back to C) | ARMv8.0 baseline (tested on Cortex-A53) |

- **Pick the build in the setup wizard**: the setup wizard and the "FFmpeg preference" dialog offer a track × variant selector on the download step (track: default / `-legacy`; variant: static / `-shared` / `-debug` / `-shared-debug`). On an x86_64 host whose CPU predates Haswell (no AVX2), simply select the `-legacy` track in the wizard — no manual file placement needed. The installed copy's `ffmpeg-build.json` marker records the chosen track and variant, and idle auto-update always downloads the same build again.
- **Manual placement still works**: extract the desired build into `tools/` (`./data/tools` when mounted in Docker); without an `ffmpeg-build.json` marker it counts as manually placed and idle auto-update never replaces it. A system-PATH FFmpeg works too (selecting the PATH source disables auto-update anyway).
- **Docker**: the image does not bundle FFmpeg — the setup wizard downloads on first start; simply pick the `-legacy` track in the wizard, no file replacement inside the container needed.
- arm64 hosts (Raspberry Pi, ARM VPS, etc.) can run either track; Windows and macOS have only the default track — none of these need any special handling.

---

## Building from Source

**Prerequisites:** Rust, Node.js (LTS), ffmpeg (optional: if missing, the program automatically downloads it into the `tools` folder next to the program on startup)

### First-launch Setup

When running the binary directly, if the setup wizard has not been completed in `config/settings.json`, a web-based setup wizard will appear on first visit:

1. Select UI language
2. Set the recording output directory
3. Configure network proxy (optional)
4. FFmpeg check (pick the source location; if `tools` is incomplete a download can be triggered in place — track and variant selectable — with progress shown inline)
5. Set the admin password

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
- **Container:** Debian (FFmpeg downloaded and managed by the program at runtime, not baked in)

---

## License

This project is licensed under the [GNU General Public License v3.0](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html).

---

## Disclaimer

This project is intended for technical research and learning only. Users are responsible for deployment, operations, and compliance risks.
