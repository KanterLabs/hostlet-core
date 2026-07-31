FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release -p hostlet-agent

FROM debian:bookworm-slim
ARG TARGETARCH=amd64
ARG DOCKER_CLI_VERSION=29.1.3
ARG DOCKER_BUILDX_VERSION=0.34.1
ARG DOCKER_COMPOSE_VERSION=2.40.3
ARG RAILPACK_VERSION=0.25.0
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl git tar \
  && case "${TARGETARCH}" in \
    amd64) docker_arch="x86_64"; buildx_arch="amd64"; compose_arch="x86_64"; railpack_arch="x86_64"; docker_sha="c019c608ba2bb009dd673f3230e4d743f36a78d36166c6c2444c05d0aa9ff0d9"; buildx_sha="f1332ddb9010bd0b72628266c3a906d9a6979848033df4c8d9bd2cd113bae12b"; compose_sha="dba9d98e1ba5bfe11d88c99b9bd32fc4a0624a30fafe68eea34d61a3e42fd372"; railpack_sha="1a3e471b8b5a2f214164fe2217a3e834ef921ee1a277fd70108a51c8cb42b6cf" ;; \
    arm64) docker_arch="aarch64"; buildx_arch="arm64"; compose_arch="aarch64"; railpack_arch="arm64"; docker_sha="2219f3ac48727d8c5546b13d5d271a58856188ecb8ff1f71f9e2f9aefde3ea76"; buildx_sha="c34e32dd6ea2653d960d6c099c9f09b9077e4a37504d2d31e5066eccc3904231"; compose_sha="d26373b19e89160546d15407516cc59f453030d9bc5b43ba7faf16f7b4980137"; railpack_sha="e428c9a3bd7d237f4b53d683e67204c366868304c4f1b4d01befdfff15215b5c" ;; \
    *) echo "Unsupported Docker CLI architecture: ${TARGETARCH}" >&2; exit 1 ;; \
  esac \
  && curl -fsSL "https://download.docker.com/linux/static/stable/${docker_arch}/docker-${DOCKER_CLI_VERSION}.tgz" -o /tmp/docker.tgz \
  && echo "${docker_sha}  /tmp/docker.tgz" | sha256sum -c - \
  && tar -xzf /tmp/docker.tgz -C /tmp \
  && install -m 0755 /tmp/docker/docker /usr/local/bin/docker \
  && rm -rf /tmp/docker /tmp/docker.tgz \
  && install -d /usr/libexec/docker/cli-plugins \
  && curl -fsSL "https://github.com/docker/buildx/releases/download/v${DOCKER_BUILDX_VERSION}/buildx-v${DOCKER_BUILDX_VERSION}.linux-${buildx_arch}" -o /usr/libexec/docker/cli-plugins/docker-buildx \
  && echo "${buildx_sha}  /usr/libexec/docker/cli-plugins/docker-buildx" | sha256sum -c - \
  && chmod +x /usr/libexec/docker/cli-plugins/docker-buildx \
  && curl -fsSL "https://github.com/docker/compose/releases/download/v${DOCKER_COMPOSE_VERSION}/docker-compose-linux-${compose_arch}" -o /usr/libexec/docker/cli-plugins/docker-compose \
  && echo "${compose_sha}  /usr/libexec/docker/cli-plugins/docker-compose" | sha256sum -c - \
  && chmod +x /usr/libexec/docker/cli-plugins/docker-compose \
  && curl -fsSL "https://github.com/railwayapp/railpack/releases/download/v${RAILPACK_VERSION}/railpack-v${RAILPACK_VERSION}-${railpack_arch}-unknown-linux-musl.tar.gz" -o /tmp/railpack.tgz \
  && echo "${railpack_sha}  /tmp/railpack.tgz" | sha256sum -c - \
  && tar -xzf /tmp/railpack.tgz -C /tmp \
  && install -m 0755 /tmp/railpack /usr/local/bin/railpack \
  && rm -f /tmp/railpack /tmp/railpack.tgz \
  && useradd --system --home /var/lib/hostlet --create-home --shell /usr/sbin/nologin hostlet \
  && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/hostlet-agent /usr/local/bin/hostlet-agent
RUN mkdir -p /var/lib/hostlet && chown -R hostlet:hostlet /var/lib/hostlet
USER hostlet
CMD ["hostlet-agent"]
