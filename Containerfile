# The build environment for a release binary.
#
# Lightforge links almost nothing dynamically — `ldd` on a release build shows
# `libc`, `libgcc_s` and `libm` and nothing else, because Vulkan, Wayland and
# X11 are all opened at run time rather than linked at build time. So the only
# thing this image decides is **which glibc the binary demands**, and that is
# the whole reason it exists.
#
# Built on the host instead, the binary inherits the host's glibc. On a rolling
# distro that is a version almost nobody else has: measured 2026-10-08, a build
# from this machine imported `GLIBC_2.44`, which SteamOS (~2.37) and Bazzite
# (~2.40) are both older than — so the release ran on neither.
#
# Debian 12 "bookworm" is glibc 2.36. That covers SteamOS, Bazzite, Ubuntu
# 22.04 and every current distro, and a binary built against an older glibc
# runs against newer ones — the compatibility only ever points forward.
ARG RUST_IMAGE=docker.io/library/rust:bookworm
FROM ${RUST_IMAGE}

# `pkg-config` is not needed by anything today and is here because the first
# crate that wants it fails with a message nobody enjoys reading. Everything
# else a desktop app usually needs at build time — Vulkan, Wayland, X11,
# fontconfig — is genuinely absent on purpose: see the note above.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# The target directory is kept apart from the host's. The two are built by
# different toolchains against different glibcs, and sharing one means every
# switch between them rebuilds the world.
ENV CARGO_TARGET_DIR=/src/target/container

CMD ["cargo", "build", "--release", "--locked"]
