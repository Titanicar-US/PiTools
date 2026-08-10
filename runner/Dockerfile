# syntax=docker/dockerfile:1.11

FROM docker.io/library/node:26-bookworm-slim@sha256:cd565714d4da3e84bfd341e31448f81d47c6362198f152345297c9c1154e6341 AS pi-builder

WORKDIR /opt/pitools/pi
COPY workers/pi/package.json workers/pi/package-lock.json ./
RUN npm ci --ignore-scripts \
    && npm cache clean --force
COPY workers/pi/src ./src
COPY workers/pi/tsconfig.json ./tsconfig.json
RUN ./node_modules/.bin/tsc --noEmit false --outDir dist \
    && npm prune --omit=dev

FROM docker.io/library/node:26-bookworm-slim@sha256:cd565714d4da3e84bfd341e31448f81d47c6362198f152345297c9c1154e6341

ARG PITOOLS_UID=10001
ARG PITOOLS_GID=10001

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates git openssh-client tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid "${PITOOLS_GID}" pitools \
    && useradd --uid "${PITOOLS_UID}" --gid "${PITOOLS_GID}" --create-home --shell /usr/sbin/nologin pitools \
    && mkdir -p /opt/pitools/pi /workspace \
    && chown -R pitools:pitools /opt/pitools /workspace

COPY --from=pi-builder --chown=pitools:pitools /opt/pitools/pi/package.json /opt/pitools/pi/package-lock.json /opt/pitools/pi/
COPY --from=pi-builder --chown=pitools:pitools /opt/pitools/pi/node_modules /opt/pitools/pi/node_modules
COPY --from=pi-builder --chown=pitools:pitools /opt/pitools/pi/dist /opt/pitools/pi/dist

ENV HOME=/home/pitools \
    NODE_ENV=production \
    TMPDIR=/tmp

WORKDIR /workspace
USER 10001:10001

ENTRYPOINT ["/usr/bin/tini", "--", "node", "/opt/pitools/pi/dist/main.js"]
