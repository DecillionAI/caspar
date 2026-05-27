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
# Downloads the binary and guest kernel at build time.
# Network bridge setup (br0 / NAT) is a host-level operation done at runtime
# by run-nodes.sh — it is NOT performed here.
ARG FC_VERSION=v1.10.1
RUN apt-get update -y && apt-get install -y --no-install-recommends \
        libelf-dev e2fsprogs \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

RUN set -e; \
    FC_ARCH=$(uname -m); \
    TGZ="firecracker-${FC_VERSION}-${FC_ARCH}.tgz"; \
    curl -fsSL \
      "https://github.com/firecracker-microvm/firecracker/releases/download/${FC_VERSION}/${TGZ}" \
      -o "/tmp/${TGZ}"; \
    tar -xzf "/tmp/${TGZ}" -C /tmp; \
    mv "/tmp/release-${FC_VERSION}-${FC_ARCH}/firecracker-${FC_VERSION}-${FC_ARCH}" \
       /usr/local/bin/firecracker; \
    chmod +x /usr/local/bin/firecracker; \
    rm -rf "/tmp/${TGZ}" "/tmp/release-${FC_VERSION}-${FC_ARCH}"

RUN set -e; \
    FC_ARCH=$(uname -m); \
    mkdir -p /opt/firecracker/{vms,kernel,rootfs,snapshots}; \
    curl -fsSL \
      "https://s3.amazonaws.com/spec.ccfc.min/img/quickstart_guide/${FC_ARCH}/kernels/vmlinux.bin" \
      -o /opt/firecracker/kernel/vmlinux; \
    chmod +x /opt/firecracker/kernel/vmlinux

# ─── Entrypoint ─────────────────────────────────────────────────────────────
COPY node/scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

RUN mkdir -p /app/data

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
