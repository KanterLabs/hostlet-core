FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release -p hostlet-agent

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl docker.io git \
  && useradd --system --home /var/lib/hostlet --create-home --shell /usr/sbin/nologin hostlet \
  && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/hostlet-agent /usr/local/bin/hostlet-agent
RUN mkdir -p /var/lib/hostlet && chown -R hostlet:hostlet /var/lib/hostlet
USER hostlet
CMD ["hostlet-agent"]
