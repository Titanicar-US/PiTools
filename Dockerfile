# syntax=docker/dockerfile:1.11
FROM --platform=$BUILDPLATFORM docker.io/library/rust:1.97.1-bookworm@sha256:14bc9c5966e7b3a385794b3d5389a8765668342025fbcc7b2e3d2866ac4bd8c3 AS rust-builder

ARG TARGETARCH
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) \
        rustup target add x86_64-unknown-linux-gnu; \
        cargo build --locked --release --bin pitools --target x86_64-unknown-linux-gnu; \
        cp target/x86_64-unknown-linux-gnu/release/pitools /tmp/pitools; \
        ;; \
      arm64) \
        apt-get update; \
        apt-get install --yes --no-install-recommends gcc-aarch64-linux-gnu; \
        rm -rf /var/lib/apt/lists/*; \
        rustup target add aarch64-unknown-linux-gnu; \
        CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
        AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
          cargo build --locked --release --bin pitools --target aarch64-unknown-linux-gnu; \
        cp target/aarch64-unknown-linux-gnu/release/pitools /tmp/pitools; \
        ;; \
      *) \
        echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; \
        exit 1; \
        ;; \
    esac; \
    install -Dm755 /tmp/pitools /out/pitools

FROM docker.io/library/debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime

ARG PITOOLS_UID=10001
ARG PITOOLS_GID=10001

RUN apt-get update \
    && apt-get install --yes --no-install-recommends bubblewrap ca-certificates curl git make tar tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid "${PITOOLS_GID}" pitools \
    && useradd --uid "${PITOOLS_UID}" --gid "${PITOOLS_GID}" --create-home --shell /usr/sbin/nologin pitools

COPY --from=rust-builder /out/pitools /usr/local/bin/pitools

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
