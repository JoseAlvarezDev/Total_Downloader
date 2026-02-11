FROM rust:1.92-bookworm AS builder
WORKDIR /app

COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/src ./src

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates ffmpeg yt-dlp \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/backend /usr/local/bin/backend

ENV RUST_LOG=backend=info,tower_http=info
EXPOSE 10000

CMD ["backend"]
