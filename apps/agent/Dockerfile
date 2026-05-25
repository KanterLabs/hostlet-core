FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release -p hostlet-agent

FROM debian:bookworm-slim
ARG DOCKER_COMPOSE_VERSION=2.40.3
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl docker.io git \
  && install -d /usr/libexec/docker/cli-plugins \
  && curl -fsSL "https://github.com/docker/compose/releases/download/v${DOCKER_COMPOSE_VERSION}/docker-compose-linux-x86_64" -o /usr/libexec/docker/cli-plugins/docker-compose \
  && chmod +x /usr/libexec/docker/cli-plugins/docker-compose \
  && useradd --system --home /var/lib/hostlet --create-home --shell /usr/sbin/nologin hostlet \
  && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/hostlet-agent /usr/local/bin/hostlet-agent
RUN mkdir -p /var/lib/hostlet && chown -R hostlet:hostlet /var/lib/hostlet
USER hostlet
CMD ["hostlet-agent"]
