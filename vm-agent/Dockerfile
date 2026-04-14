FROM alpine:3.19

# Build dependencies
RUN apk add --no-cache \
    build-base \
    curl \
    wget \
    pkgconf \
    fuse3-dev \
    openssl-dev \
    openssl-libs-static \
    linux-headers \
    meson \
    ninja

# Build libfuse3.a (Alpine's fuse3-dev only ships shared libraries)
RUN wget -qO /tmp/fuse3.tar.gz \
        https://github.com/libfuse/libfuse/archive/refs/tags/fuse-3.16.2.tar.gz && \
    tar -C /tmp -xf /tmp/fuse3.tar.gz && \
    meson setup /tmp/fuse3-build /tmp/libfuse-fuse-3.16.2 \
        --buildtype=release \
        --default-library=static \
        -Dprefix=/usr \
        -Dexamples=false \
        -Dtests=false && \
    ninja -C /tmp/fuse3-build && \
    find /tmp/fuse3-build -name 'libfuse3.a' -exec cp {} /usr/lib/ \; && \
    rm -rf /tmp/fuse3-build /tmp/libfuse-fuse-3.16.2 /tmp/fuse3.tar.gz

# Install Rust (stable, minimal profile)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --profile minimal
ENV PATH="/root/.cargo/bin:$PATH"

WORKDIR /workspace
