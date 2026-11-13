# The tools build.sh needs, for a machine without them (or a Mac):
#   docker build -t pakon-win-build packaging/windows
# build.sh does this itself when MinGW, NSIS or the Rust Windows target is
# missing.
FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        --no-install-recommends ca-certificates curl git cmake make gcc libc6-dev \
        gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 binutils-mingw-w64-x86-64 \
        pkg-config libclang-dev nsis python3 python3-venv \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain 1.99.0 --target x86_64-pc-windows-gnu --no-modify-path \
    && chmod -R a+rwX /usr/local/cargo /usr/local/rustup
