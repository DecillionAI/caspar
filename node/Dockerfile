FROM ubuntu:24.04
WORKDIR /app

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update -y && apt-get upgrade -y && \
    apt-get install -y curl dpkg-dev g++ gcc git ninja-build \
    software-properties-common wget zlib1g-dev imagemagick valgrind && \
    apt-get install -y cmake && \
    apt-get install -y clang-17 liblld-17-dev libpolly-17-dev llvm-17-dev && \
    apt-get clean && \
    update-alternatives --install /usr/bin/clang clang /usr/bin/clang-17 100 && \
    update-alternatives --install /usr/bin/clang++ clang++ /usr/bin/clang++-17 100 && \
    update-alternatives --install /usr/bin/llvm-strip llvm-strip /usr/bin/llvm-strip-17 100

ENV CC=/usr/bin/clang-17
ENV CXX=/usr/bin/clang++-17

RUN curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash -s -- -v 0.14.0

ENV PATH="/root/.wasmedge/bin:$PATH"
ENV LD_LIBRARY_PATH="/root/.wasmedge/lib:$LD_LIBRARY_PATH"
ENV LIBRARY_PATH="/root/.wasmedge/lib:$LIBRARY_PATH"
ENV C_INCLUDE_PATH="/root/.wasmedge/include:$C_INCLUDE_PATH"
ENV CPLUS_INCLUDE_PATH="/root/.wasmedge/include:$CPLUS_INCLUDE_PATH"
ENV WASMEDGE_LIB_DIR="/root/.wasmedge/lib"

RUN apt install -y libgflags-dev libsnappy-dev libbz2-dev liblz4-dev libnuma-dev libzmq3-dev

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:$PATH"

COPY . /app

RUN mkdir -p /app/storage

RUN bash node/scripts/install-fcvmm.sh

RUN apt install -y openjdk-17-jdk openjdk-17-jre
ENV JAVA_HOME="/usr/lib/jvm/java-17-openjdk-arm64"
ENV PATH=$JAVA_HOME/bin:$PATH

RUN bash node/scripts/install-questdb.sh
ENV PKG_CONFIG_PATH="$PKG_CONFIG_PATH:/usr/local/lib/pkgconfig"

# Build the Rust appengine that the vmm driver bridges to over ZMQ.
WORKDIR /app/node/crates/appengine
RUN cargo build --release

# Build the node binary, the keygen helper, and the casparctl CLI.
WORKDIR /app
RUN cargo build --release --manifest-path node/Cargo.toml
RUN cargo build --release --manifest-path cmd/casparctl/Cargo.toml
RUN cp node/target/release/caspar-node    /app/kasper
RUN cp node/target/release/caspar-keygen  /app/caspar-keygen
RUN cp cmd/casparctl/target/release/casparctl /app/casparctl

ENTRYPOINT ["bash", "/app/node/scripts/run.sh"]
