# syntax=docker/dockerfile:1.11

FROM docker.io/library/rust:1.97.1-bookworm@sha256:14bc9c5966e7b3a385794b3d5389a8765668342025fbcc7b2e3d2866ac4bd8c3 AS rust-builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --locked --release --bin pitools

FROM docker.io/library/debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime

ARG PITOOLS_UID=10001
ARG PITOOLS_GID=10001

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl git make tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid "${PITOOLS_GID}" pitools \
    && useradd --uid "${PITOOLS_UID}" --gid "${PITOOLS_GID}" --create-home --shell /usr/sbin/nologin pitools

COPY --from=rust-builder /build/target/release/pitools /usr/local/bin/pitools

RUN mkdir -p /var/lib/pitools \
    && chown pitools:pitools /var/lib/pitools \
    && chmod 0555 /usr/local/bin/pitools

ENV HOME=/home/pitools \
    PITOOLS_BIND_ADDRESS=0.0.0.0:8080 \
    TMPDIR=/tmp

WORKDIR /var/lib/pitools
USER 10001:10001
EXPOSE 8080

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/pitools", "server"]
