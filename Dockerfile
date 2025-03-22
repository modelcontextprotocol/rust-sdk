FROM rust:1.85 AS builder
WORKDIR /app

COPY . .
RUN cargo build  --release --example counter-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/examples/counter-server ./counter-server

USER root

CMD ["./counter-server"]