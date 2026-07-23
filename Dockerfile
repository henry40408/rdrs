# syntax=docker/dockerfile:1

# ---- build: cross-compile a static musl binary with cargo-zigbuild ----------
# The builder is pinned to the native build platform; zig cross-compiles to the
# target arch's musl triple, so no qemu emulation is needed — an arm64 image
# builds at the host's native speed. The C dependencies — vendored OpenSSL
# (pulled in by webauthn-rs), bundled SQLite, and mimalloc — are all compiled by
# zig cc. OpenSSL's build driver (perl + make) and the favicon renderer (resvg,
# pure Rust) need no extra system packages: perl and make already ship in the
# rust:bookworm (buildpack-deps) base, so only zig has to be fetched.
FROM --platform=$BUILDPLATFORM rust:1.96-bookworm AS build

# curl + xz fetch zig; that is the only build-time system dependency.
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Zig 0.14.1 avoids the libc++-19 bindgen requirement that 0.15+ introduces.
ARG ZIG_VERSION=0.14.1
ARG ZIGBUILD_VERSION=0.22.3
RUN cargo install cargo-zigbuild --version "${ZIGBUILD_VERSION}" --locked
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64) zarch=x86_64 ;; \
      aarch64) zarch=aarch64 ;; \
      *) echo "unsupported build arch $(uname -m)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-${zarch}-linux-${ZIG_VERSION}.tar.xz" \
      | tar -xJ -C /opt; \
    ln -s "/opt/zig-${zarch}-linux-${ZIG_VERSION}/zig" /usr/local/bin/zig

WORKDIR /app
COPY . .

# Map Docker's TARGETARCH onto the Rust musl triple and build. `rustup target
# add` runs after the source (and rust-toolchain.toml) is in place, so it
# resolves against the pinned toolchain rather than the base image's default.
# .git is excluded by .dockerignore, so build.rs reads the version from the
# GIT_VERSION build arg the CI workflow passes.
ARG TARGETARCH
ARG GIT_VERSION=dev
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target,sharing=locked \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported target arch $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    rustup target add "$target"; \
    GIT_VERSION="${GIT_VERSION}" cargo zigbuild --release --target "$target"; \
    install -Dm755 "target/${target}/release/rdrs" /out/rdrs

# ---- runtime: minimal static image (CA certs + tzdata, no shell) ------------
# distroless/static (not :nonroot) keeps the root runtime user the previous
# distroless/cc image defaulted to, so the bind-mounted /data SQLite file stays
# writable without a permissions change.
FROM gcr.io/distroless/static-debian12

COPY --from=build /out/rdrs /rdrs

VOLUME /data

ENV DATABASE_URL=/data/rdrs.sqlite3
ENV RDRS_SERVER_BIND=0.0.0.0:8080

EXPOSE 8080

ENTRYPOINT ["/rdrs"]
