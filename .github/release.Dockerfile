FROM rockylinux:8

RUN dnf install -y --setopt=install_weak_deps=False \
        binutils ca-certificates curl gcc gcc-c++ make pkgconf-pkg-config \
        libX11-devel libXcursor-devel libXi-devel libXrandr-devel \
        libxkbcommon-devel mesa-libGL-devel wayland-devel \
    && dnf clean all

ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.89.0 \
    && rm /tmp/rustup-init.sh

WORKDIR /work
