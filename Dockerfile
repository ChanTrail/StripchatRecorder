FROM debian:latest AS builder

LABEL maintainer="chantrail@chantrail.com" \
      version="0.2.0" \
      description="Stripchat Recorder Docker builder for Debian"

RUN sed -i 's/deb.debian.org/mirrors.ustc.edu.cn/g' /etc/apt/sources.list.d/debian.sources

RUN apt-get update && apt-get install -y \
    curl \
    wget \
    git \
    libglib2.0-dev \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    libpango1.0-dev \
    libcairo2-dev \
    libgdk-pixbuf-xlib-2.0-dev \
    libsoup-3.0-dev \
    pkg-config \
    build-essential \
    libssl-dev \
    xdg-utils \
    libfuse2 \
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

RUN ls && . /root/.cargo/env && \
    npm install && \
    npm run build && \
    npx tauri build --no-bundle && \
    mkdir -p /build/modules_dist && \
    for module_dir in /build/modules/*; do \
        if [ -f "$module_dir/Cargo.toml" ]; then \
            cargo build --manifest-path "$module_dir/Cargo.toml" --release --bins; \
            for bin_file in "$module_dir"/target/release/*; do \
                if [ -f "$bin_file" ] && [ -x "$bin_file" ]; then \
                    cp -f "$bin_file" /build/modules_dist/; \
                fi; \
            done; \
        fi; \
    done

FROM debian:latest

LABEL maintainer="chantrail@chantrail.com" \
      version="0.1.4" \
      description="Stripchat Recorder Docker image for Debian"

RUN sed -i 's/deb.debian.org/mirrors.ustc.edu.cn/g' /etc/apt/sources.list.d/debian.sources

RUN apt-get update && apt-get install -y \
    ffmpeg \
    ca-certificates \
    libssl3 \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /app /app/stripchat-recorder/logs /app/stripchat-recorder/recordings /app/stripchat-recorder/modules.default /app/stripchat-recorder/modules /app/stripchat-recorder/config /app/stripchat-recorder/config.default
WORKDIR /app

COPY --from=builder /build/src-tauri/target/release/stripchat-recorder /app/stripchat-recorder/
COPY --from=builder /build/modules_dist/ /app/stripchat-recorder/modules.default/


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
    '  "run_mode": "server",' \
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

VOLUME ["/app/stripchat-recorder/logs", "/app/stripchat-recorder/recordings", "/app/stripchat-recorder/modules.default", "/app/stripchat-recorder/modules" , "/app/stripchat-recorder/config"]

EXPOSE ${PORT:-3030}

ENTRYPOINT ["/entrypoint.sh"]
