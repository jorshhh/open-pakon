# The tools build-deb.sh needs, for a machine without them (or a Mac):
#   docker build -t pakon-deb-build packaging/linux
# build-deb.sh does this itself when cargo or WebKitGTK's headers are missing.
# Ubuntu 22.04, so the app runs on glibc 2.35 and newer (Ubuntu 22.04+,
# Debian 12+), all with WebKitGTK 4.1.
FROM ubuntu:22.04
# another Ubuntu mirror, where archive.ubuntu.com is out of reach
# (build-deb.sh passes PAKON_UBUNTU_MIRROR, e.g. https://mirrors.edge.kernel.org/ubuntu)
ARG UBUNTU_MIRROR=
RUN if [ -n "$UBUNTU_MIRROR" ]; then sed -i -E \
        "s#https?://(archive|security)\.ubuntu\.com/ubuntu/?#$UBUNTU_MIRROR#" /etc/apt/sources.list; fi
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        --no-install-recommends ca-certificates curl git build-essential cmake \
        pkg-config dpkg-dev file libclang-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain 1.99.0 --no-modify-path \
    && chmod -R a+rwX /usr/local/cargo /usr/local/rustup
