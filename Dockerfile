FROM debian:latest AS builder

LABEL maintainer="chantrail@chantrail.com" \
      version="0.4.3-beta" \
      description="Stripchat Recorder Docker builder"

RUN apt-get update && apt-get install -y \
    curl \
    pkg-config \
    build-essential \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - && \
    apt-get install -y nodejs && \
    rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && \
    . /root/.cargo/env && \
    rustup target add x86_64-unknown-linux-gnu

WORKDIR /app
COPY . /app

RUN . /root/.cargo/env && npm run build

# ── Runtime image ──────────────────────────────────────────────────────────────
FROM debian:latest

LABEL maintainer="chantrail@chantrail.com" \
      version="0.4.3-beta" \
      description="Stripchat Recorder"

RUN apt-get update && apt-get install -y \
    ffmpeg \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /app/stripchat-recorder/logs \
             /app/stripchat-recorder/ts_fragment \
             /app/stripchat-recorder/recordings \
             /app/stripchat-recorder/modules.default \
             /app/stripchat-recorder/modules \
             /app/stripchat-recorder/meta \
             /app/stripchat-recorder/config
WORKDIR /app

COPY --from=builder /app/build/stripchat-recorder /app/stripchat-recorder/
COPY --from=builder /app/build/modules/ /app/stripchat-recorder/modules.default/

RUN chmod +x /app/stripchat-recorder/stripchat-recorder

RUN printf '%s\n' \
    '#!/bin/sh' \
    'set -eu' \
    '' \
    'cp -an /app/stripchat-recorder/modules.default/. /app/stripchat-recorder/modules/' \
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

VOLUME ["/app/stripchat-recorder/logs", "/app/stripchat-recorder/ts_fragment", "/app/stripchat-recorder/recordings", "/app/stripchat-recorder/modules", "/app/stripchat-recorder/meta", "/app/stripchat-recorder/config"]

EXPOSE ${PORT:-3030}

ENTRYPOINT ["/entrypoint.sh"]
