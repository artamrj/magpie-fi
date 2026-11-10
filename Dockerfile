# syntax=docker/dockerfile:1.12
#
# The magpie-fi image: the program, its web page, and every tool it
# calls (ffmpeg, fpcalc, SongRec, rsgain, yt-dlp + Deno). Debian because
# yt-dlp and Deno need glibc.
#
# Base images come from mirror.gcr.io, Google's public copy of Docker Hub,
# which is not affected by Docker Hub's anonymous pull limits.

FROM mirror.gcr.io/library/node:26-trixie-slim AS web
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci
COPY web/ ./
RUN npm run build

# yt-dlp (the zipapp release, which can update itself) and Deno, the JavaScript
# runtime yt-dlp needs for YouTube. Both are checked against published checksums.
FROM mirror.gcr.io/library/debian:trixie-slim AS downloads
ARG TARGETARCH
ARG YTDLP_VERSION=2026.10.26
ARG DENO_VERSION=2.9.8
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl unzip && \
    rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    case "${TARGETARCH:-amd64}" in \
      amd64) deno_target=x86_64-unknown-linux-gnu ;; \
      arm64) deno_target=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported architecture ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    base=https://github.com/yt-dlp/yt-dlp/releases/download/${YTDLP_VERSION}; \
    curl -fsSL -o /tmp/yt-dlp "$base/yt-dlp"; \
    curl -fsSL "$base/SHA2-256SUMS" | grep ' yt-dlp$' | sed 's# yt-dlp$# /tmp/yt-dlp#' | sha256sum -c -; \
    install -m 755 /tmp/yt-dlp /out-yt-dlp; \
    deno=https://github.com/denoland/deno/releases/download/v${DENO_VERSION}/deno-${deno_target}.zip; \
    curl -fsSL -o /tmp/deno.zip "$deno"; \
    echo "$(curl -fsSL "$deno.sha256sum" | cut -d' ' -f1)  /tmp/deno.zip" | sha256sum -c -; \
    unzip -q /tmp/deno.zip -d /tmp/deno; \
    install -m 755 /tmp/deno/deno /out-deno

FROM mirror.gcr.io/library/rust:1.98-slim-trixie AS rust-base

# SongRec's command-line client (Shazam recognition), built once. Only this
# build needs the ALSA headers.
FROM rust-base AS songrec
RUN apt-get update && \
    apt-get install -y --no-install-recommends libasound2-dev pkg-config && \
    rm -rf /var/lib/apt/lists/*
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/songrec,sharing=locked \
    CARGO_TARGET_DIR=/build/songrec \
    cargo install songrec-lib --version 0.5.3 --locked --root /opt/songrec

FROM rust-base AS chef
WORKDIR /build
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo install cargo-chef --version 0.1.77 --locked

# A dependency-only recipe: source changes keep the cooked dependency layer.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS backend
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo chef cook --locked --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY migrations/ ./migrations/
COPY src/ ./src/
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --locked --release && \
    cp target/release/magpie-fi /tmp/magpie-fi

FROM mirror.gcr.io/library/debian:trixie-slim AS runtime
ARG VERSION=dev
ARG VCS_REF=unknown
ARG SOURCE_URL=https://github.com/artamrj/magpie-fi
LABEL org.opencontainers.image.title="magpie-fi" \
      org.opencontainers.image.description="Your own music service: identifies, tags and files every song for Navidrome" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.source="${SOURCE_URL}" \
      org.opencontainers.image.licenses="MIT"

RUN apt-get update && \
    apt-get upgrade -y --no-install-recommends && \
    apt-get install -y --no-install-recommends \
      ca-certificates ffmpeg libasound2t64 libchromaprint-tools python3 rsgain tini util-linux wget && \
    rm -rf /var/lib/apt/lists/* && \
    command -v ffmpeg && command -v ffprobe && command -v fpcalc && \
    command -v python3 && command -v rsgain && command -v setpriv && command -v tini && \
    groupadd --system --gid 10001 magpie-fi && \
    useradd --system --no-create-home --uid 10001 --gid magpie-fi magpie-fi && \
    mkdir -p /data /music /usr/share/magpie-fi

COPY --from=backend /tmp/magpie-fi /usr/local/bin/magpie-fi
COPY --from=songrec /opt/songrec/bin/songrec-lib-cli /usr/local/bin/songrec-lib-cli
COPY --from=web /build/web/dist/ /usr/share/magpie-fi/
COPY --from=downloads /out-yt-dlp /usr/local/bin/yt-dlp
COPY --from=downloads /out-deno /usr/local/bin/deno
COPY --chmod=755 scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

RUN songrec-lib-cli --help >/dev/null && \
    yt-dlp --version && deno --version && \
    sh -n /usr/local/bin/docker-entrypoint.sh

ENV PUID=10001 \
    PGID=10001 \
    HOME=/tmp \
    MAGPIE_BIND=0.0.0.0:7331 \
    MAGPIE_ALLOW_NON_LOOPBACK=true \
    MAGPIE_DATA_DIR=/data \
    MAGPIE_MUSIC_ROOT=/music \
    MAGPIE_WEB_DIR=/usr/share/magpie-fi \
    MAGPIE_SONGREC_BIN=/usr/local/bin/songrec-lib-cli \
    MAGPIE_YTDLP_BIN=/usr/local/bin/yt-dlp \
    MAGPIE_YTDLP_SELF_UPDATE=true \
    DENO_DIR=/tmp/deno \
    XDG_CACHE_HOME=/tmp/.cache \
    RUST_LOG=info

EXPOSE 7331
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["wget", "-q", "-O", "/dev/null", "http://127.0.0.1:7331/api/health"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/magpie-fi"]
