# =============================================================================
# Caspar Node — pre-built image
#
# IMPORTANT: Run `./build-dist.sh` at the repo root BEFORE `docker build`.
# That script compiles caspar-node, casparctl, and copies all native libraries
# into dist/.  This Dockerfile copies those artifacts directly, so the build
# takes ~30s instead of ~45 min.
#
# Build:
#   ./build-dist.sh
#   docker build -f node/Dockerfile -t caspar-node:latest .
#
# Run (single node, minimal):
#   docker run -d \
#     --env-file /tmp/caspar/node1/.env \
#     -e BABBLE_DIR=/app/data/babble \
#     -p 8074:8074 -p 8076:8076 -p 8077:8077 \
#     caspar-node:latest
# =============================================================================

FROM ubuntu:24.04
WORKDIR /app

ENV DEBIAN_FRONTEND=noninteractive

# ─── Runtime dependencies only (no compiler, no Rust) ───────────────────────
# Everything else is statically linked into caspar-node.
RUN apt-get update -y && apt-get install -y --no-install-recommends \
    libstdc++6 \
    zlib1g \
    libtinfo6 \
    ca-certificates \
    curl \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# ─── WasmEdge runtime library (pre-built, from dist/lib/wasmedge/) ──────────
COPY dist/lib/wasmedge/ /usr/local/lib/wasmedge/

RUN echo "/usr/local/lib/wasmedge" > /etc/ld.so.conf.d/wasmedge.conf \
 && ldconfig

ENV LD_LIBRARY_PATH="/usr/local/lib/wasmedge"
ENV WASMEDGE_LIB_DIR="/usr/local/lib/wasmedge"

# ─── Node binaries (pre-built, from dist/bin/) ──────────────────────────────
COPY dist/bin/caspar-node   /usr/local/bin/caspar-node
COPY dist/bin/caspar-keygen /usr/local/bin/caspar-keygen
COPY dist/bin/casparctl      /usr/local/bin/casparctl

RUN chmod +x /usr/local/bin/caspar-node \
             /usr/local/bin/caspar-keygen \
             /usr/local/bin/casparctl

# ─── Java (for QuestDB telemetry process) ───────────────────────────────────
RUN apt-get update -y && apt-get install -y --no-install-recommends \
    openjdk-17-jre-headless \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# ─── QuestDB telemetry server (pre-fetched, from dist/questdb/) ─────────────
COPY dist/questdb/questdb.jar /opt/questdb/questdb.jar

# ─── Firecracker microVM manager ────────────────────────────────────────────
COPY node/scripts/install-fcvmm.sh /tmp/install-fcvmm.sh
RUN apt-get update -y && apt-get install -y --no-install-recommends \
    make git gcc libelf-dev pkg-config lsof screen net-tools \
    && bash /tmp/install-fcvmm.sh \
    && apt-get clean && rm -rf /var/lib/apt/lists/* \
    && rm /tmp/install-fcvmm.sh

# ─── Entrypoint ─────────────────────────────────────────────────────────────
COPY node/scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

RUN mkdir -p /app/data

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
