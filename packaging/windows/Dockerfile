# The tools build.sh needs, for a Linux machine without them:
#   docker build -t pakon-win-build packaging/windows
# build.sh does this itself when MinGW or NSIS is missing.
FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        --no-install-recommends ca-certificates curl git cmake make \
        gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64 pkg-config nsis p7zip-full unzip \
        python3 python3-venv \
    && rm -rf /var/lib/apt/lists/*
