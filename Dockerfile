FROM debian:latest AS builder

LABEL maintainer="chantrail@chantrail.com" \
      version="0.3.0" \
      description="Stripchat Recorder Docker builder"

RUN sed -i 's/deb.debian.org/mirrors.ustc.edu.cn/g' /etc/apt/sources.list.d/debian.sources

RUN apt-get update && apt-get install -y \
    curl \
    pkg-config \
    build-essential \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - && \
    apt-get install -y nodejs && \
    rm -rf /var/lib/apt/lists/*

RUN npm config set registry https://registry.npmmirror.com

ENV RUSTUP_DIST_SERVER=https://mirrors.ustc.edu.cn/rust-static \
    RUSTUP_UPDATE_ROOT=https://mirrors.ustc.edu.cn/rust-static/rustup

RUN curl --proto '=https' --tlsv1.2 -sSf https://mirrors.ustc.edu.cn/misc/rustup-install.sh | sh -s -- -y && \
    . /root/.cargo/env && \
    rustup target add x86_64-unknown-linux-gnu

RUN mkdir -vp ${CARGO_HOME:-$HOME/.cargo} && \
    printf '%s\n' \
    '[source.crates-io]' \
    "replace-with = 'ustc'" \
    '' \
    '[source.ustc]' \
    'registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"' \
    '' \
    '[registries.ustc]' \
    'index = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"' \
    | tee -a ${CARGO_HOME:-$HOME/.cargo}/config.toml

WORKDIR /build
COPY . /build

RUN . /root/.cargo/env && \
    # Run the unified release script: builds frontend + backend + modules,
    # collects binaries into /build/build/, then removes /build/build_tmp/
    cd /build && node scripts/release.js

# ── Runtime image ──────────────────────────────────────────────────────────────
FROM debian:latest

LABEL maintainer="chantrail@chantrail.com" \
      version="0.3.0" \
      description="Stripchat Recorder"

RUN sed -i 's/deb.debian.org/mirrors.ustc.edu.cn/g' /etc/apt/sources.list.d/debian.sources

RUN apt-get update && apt-get install -y \
    ffmpeg \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /app/stripchat-recorder/logs \
             /app/stripchat-recorder/recordings \
             /app/stripchat-recorder/modules.default \
             /app/stripchat-recorder/modules \
             /app/stripchat-recorder/config \
             /app/stripchat-recorder/config.default
WORKDIR /app

COPY --from=builder /build/build/stripchat-recorder /app/stripchat-recorder/
COPY --from=builder /build/build/modules/ /app/stripchat-recorder/modules.default/

RUN chmod +x /app/stripchat-recorder/stripchat-recorder

RUN printf '%s\n' \
    '{' \
    '  "output_dir": "/app/stripchat-recorder/recordings",' \
    '  "poll_interval_secs": 30,' \
    '  "auto_record": true,' \
    '  "api_proxy_url": null,' \
    '  "cdn_proxy_url": null,' \
    '  "sc_mirror_url": null,' \
    '  "max_concurrent": 0,' \
    '  "merge_format": "mp4",' \
    '  "language": "zh-CN",' \
    '  "server_port": 3030' \
    '}' \
    > /app/stripchat-recorder/config.default/settings.json

RUN printf '%s\n' \
    '#!/bin/sh' \
    'set -eu' \
    '' \
    'cp -an /app/stripchat-recorder/modules.default/. /app/stripchat-recorder/modules/' \
    'cp -an /app/stripchat-recorder/config.default/settings.json /app/stripchat-recorder/config/settings.json' \
    '' \
    '# Override language from LANGUAGE env var if set (e.g. LANGUAGE=en-US)' \
    'if [ -n "${LANGUAGE:-}" ]; then' \
    '    sed -i "s/\"language\": \"[^\"]*\"/\"language\": \"${LANGUAGE}\"/" /app/stripchat-recorder/config/settings.json' \
    'fi' \
    '' \
    '# Override server port from PORT env var if set (e.g. PORT=8080)' \
    'if [ -n "${PORT:-}" ]; then' \
    '    sed -i "s/\"server_port\": [0-9]*/\"server_port\": ${PORT}/" /app/stripchat-recorder/config/settings.json' \
    'fi' \
    '' \
    'exec /app/stripchat-recorder/stripchat-recorder "$@"' \
    > /entrypoint.sh && chmod +x /entrypoint.sh

VOLUME ["/app/stripchat-recorder/logs", "/app/stripchat-recorder/recordings", "/app/stripchat-recorder/modules", "/app/stripchat-recorder/config"]

EXPOSE ${PORT:-3030}

ENTRYPOINT ["/entrypoint.sh"]
