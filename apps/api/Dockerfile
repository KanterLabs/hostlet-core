FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release -p hostlet-api

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /src/target/release/hostlet-api /usr/local/bin/hostlet-api
COPY --from=builder /src/apps/api/migrations /app/apps/api/migrations
EXPOSE 8080
CMD ["hostlet-api"]
