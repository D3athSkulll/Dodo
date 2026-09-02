# Multi-stage build. One image, both binaries (`invoice-service` and `mock-psp`);
# docker-compose picks which to run via `command:`.
#
# cargo-chef caches the dependency compile as its own layer, so day-to-day source
# changes rebuild in seconds. TLS is rustls everywhere (reqwest, sqlx), so there
# is no OpenSSL / libssl-dev to install.

FROM rust:1.98-slim-bookworm AS chef
RUN cargo install cargo-chef --version 0.1.71 --locked
WORKDIR /app

# --- plan: hash the manifests into a dependency recipe ---
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- build: cook deps from the recipe (cached), then the workspace ---
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# `sqlx::migrate!()` embeds the migration SQL at compile time, so the runtime
# image needs no migrations directory. All queries are unchecked, so the build
# needs no database and no .sqlx cache.
RUN cargo build --release --workspace

# --- runtime: slim, non-root, CA certs for outbound HTTPS webhooks ---
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home app
COPY --from=builder /app/target/release/invoice-service /usr/local/bin/invoice-service
COPY --from=builder /app/target/release/mock-psp /usr/local/bin/mock-psp
USER app
CMD ["invoice-service"]
