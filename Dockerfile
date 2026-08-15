# ShinyProxy (Rust) — a single binary in a small image.
#
#   docker build -t shinyproxy-rust .
#   docker run --rm -p 8080:8080 \
#       -v /var/run/docker.sock:/var/run/docker.sock \
#       -v "$PWD/application.yml:/opt/shinyproxy/application.yml:ro" \
#       shinyproxy-rust
#
# The configuration is read from /opt/shinyproxy/application.yml (the working directory), so mounting a file
# there is enough; every property can also be given as an environment variable (PROXY_PORT, ...) or as a
# command line argument (--proxy.port=8080).

FROM rust:1.97-slim-bookworm AS build

# the crates need a linker, OpenSSL headers (bollard/reqwest use rustls, but some transitive crates probe
# for pkg-config) and git for the build stamp
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# the dependencies are built first, so a change in the sources does not rebuild the world
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/spel/Cargo.toml crates/spel/Cargo.toml
COPY crates/containerproxy/Cargo.toml crates/containerproxy/Cargo.toml
COPY crates/shinyproxy/Cargo.toml crates/shinyproxy/Cargo.toml
COPY crates/testapp/Cargo.toml crates/testapp/Cargo.toml
RUN mkdir -p crates/spel/src crates/containerproxy/src crates/shinyproxy/src crates/testapp/src \
    && echo "fn main() {}" > crates/shinyproxy/src/main.rs \
    && echo "fn main() {}" > crates/testapp/src/main.rs \
    && touch crates/spel/src/lib.rs crates/containerproxy/src/lib.rs crates/shinyproxy/src/lib.rs \
    && cargo build --release --bin shinyproxy 2>/dev/null || true

COPY . .
# the commit is passed in because .dockerignore keeps .git out of the build context
ARG GIT_COMMIT=unknown
ENV SHINYPROXY_GIT_COMMIT=$GIT_COMMIT
# the layer above left stale fingerprints for the workspace crates
RUN touch crates/*/src/lib.rs crates/shinyproxy/src/main.rs \
    && cargo build --release --bin shinyproxy \
    && strip target/release/shinyproxy || true

FROM debian:bookworm-slim AS runtime

# TLS roots for the OpenID Connect/LDAP/registry connections; nothing else is needed
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 shinyproxy \
    && useradd --system --uid 1000 --gid 1000 --home-dir /opt/shinyproxy shinyproxy \
    && mkdir -p /opt/shinyproxy \
    && chown shinyproxy:shinyproxy /opt/shinyproxy

COPY --from=build /src/target/release/shinyproxy /usr/local/bin/shinyproxy

USER shinyproxy
WORKDIR /opt/shinyproxy

# the application port and the management port (`management.server.port`)
EXPOSE 8080 9090

ENTRYPOINT ["/usr/local/bin/shinyproxy"]
