# The build environment for a release binary.
#
# Lightforge links almost nothing dynamically — `ldd` on a release build shows
# `libc`, `libgcc_s`, `libm` and `libudev` (gamepads, through gilrs) and
# nothing else, because Vulkan, Wayland and X11 are all opened at run time
# rather than linked at build time. So the only
# thing this image decides is **which glibc the binary demands**, and that is
# the whole reason it exists.
#
# Built on the host instead, the binary inherits the host's glibc. On a rolling
# distro that is a version almost nobody else has: measured 2026-10-08, a build
# from this machine imported `GLIBC_2.44`, which SteamOS (~2.37) and Bazzite
# (~2.40) are both older than — so the release ran on neither.
#
# Debian 12 "bookworm" is glibc 2.36, though the binaries only use symbols up
# to 2.35 (`make check-glibc` holds them there), so Ubuntu 22.04 runs them
# too. A binary built against an older glibc runs against newer ones — the
# compatibility only ever points forward.
ARG RUST_IMAGE=docker.io/library/rust:bookworm
FROM ${RUST_IMAGE}

# `libudev-dev` is for gilrs, which finds it through `pkg-config`. Everything
# else a desktop app usually needs at build time — Vulkan, Wayland, X11,
# fontconfig — is genuinely absent on purpose: see the note above.
# `squashfs-tools` packs the AppImage (`make appimage`).
#
# The aarch64 release is cross-compiled from here too: Debian's cross gcc for
# `ring`'s C and the link, the arm64 libudev through multiarch, and
# `qemu-user` to ask the result for its `--version`.
RUN dpkg --add-architecture arm64 \
    && apt-get update \
    && apt-get install --yes --no-install-recommends pkg-config libudev-dev squashfs-tools \
       gcc-aarch64-linux-gnu libc6-dev-arm64-cross libudev-dev:arm64 qemu-user \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add aarch64-unknown-linux-gnu
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    PKG_CONFIG_PATH_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig \
    PKG_CONFIG_ALLOW_CROSS=1

WORKDIR /src

# The target directory is kept apart from the host's. The two are built by
# different toolchains against different glibcs, and sharing one means every
# switch between them rebuilds the world.
ENV CARGO_TARGET_DIR=/src/target/container

CMD ["cargo", "build", "--release", "--locked"]
