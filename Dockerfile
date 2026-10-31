# kraken-trading-bot
#
#   docker compose up -d --build
#
# The image only holds the binary: the managers are in /config/managers.yaml (mounted), the
# SQLite database in /data (volume), the credentials in the environment.

# ---- build ----
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked -p kraken-trading-bot \
    && cp target/release/kraken-trading-bot /kraken-trading-bot \
    && mkdir -p /out/data /out/config

# ---- run ----
# no shell, no package manager, non-root user (65532); rustls embeds its root certificates
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /kraken-trading-bot /usr/local/bin/kraken-trading-bot
# owned by the user of the image: a named volume mounted on /data inherits it
COPY --from=builder --chown=65532:65532 /out/data /data
COPY --from=builder --chown=65532:65532 /out/config /config
ENV KRAKEN_BOT_CONFIG=/config/managers.yaml \
    KRAKEN_BOT_DATABASE=/data/kraken-bot.sqlite \
    RUST_LOG=info
VOLUME ["/data"]
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/kraken-trading-bot"]
