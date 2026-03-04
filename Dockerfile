FROM rust:1.91-slim-bookworm AS planner
WORKDIR /app
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM rust:1.91-slim-bookworm AS builder
WORKDIR /app
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json

RUN apt update -y && apt install -y cmake && rm -rf /var/lib/apt/lists/*

RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin text-to-speech-rs

FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/text-to-speech-rs /app/text-to-speech-rs
RUN useradd -m -u 1000 nonroot
USER nonroot:nonroot

ENTRYPOINT ["/app/text-to-speech-rs"]