# Stage 1: Chef - prepare recipe (runs on build platform)
FROM --platform=$BUILDPLATFORM rust:1.94-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

# Stage 2: Planner - create recipe.json
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder - compile with musl for fully static binary
FROM chef AS builder

# Target platform args (set by docker buildx)
ARG TARGETPLATFORM
ARG GIT_VERSION=dev

# Install musl toolchain
# For amd64: musl-tools provides musl-gcc
# For arm64: download pre-built musl cross-compiler
RUN apt-get update && apt-get install -y musl-tools && \
    case "$TARGETPLATFORM" in \
        "linux/arm64") \
            curl -fsSL https://musl.cc/aarch64-linux-musl-cross.tgz | tar xz -C /opt && \
            ln -s /opt/aarch64-linux-musl-cross/bin/aarch64-linux-musl-gcc /usr/local/bin/ && \
            ln -s /opt/aarch64-linux-musl-cross/bin/aarch64-linux-musl-ar /usr/local/bin/ && \
            rustup target add aarch64-unknown-linux-musl \
            ;; \
        "linux/amd64"|*) \
            rustup target add x86_64-unknown-linux-musl \
            ;; \
    esac

# Set the Rust target and CC/linker based on platform
RUN case "$TARGETPLATFORM" in \
        "linux/arm64") \
            echo "aarch64-unknown-linux-musl" > /tmp/rust_target && \
            echo "aarch64-linux-musl-gcc" > /tmp/musl_cc \
            ;; \
        "linux/amd64"|*) \
            echo "x86_64-unknown-linux-musl" > /tmp/rust_target && \
            echo "musl-gcc" > /tmp/musl_cc \
            ;; \
    esac

# Configure cargo linker for the target
RUN mkdir -p .cargo && \
    RUST_TARGET=$(cat /tmp/rust_target) && \
    MUSL_CC=$(cat /tmp/musl_cc) && \
    printf '[target.%s]\nlinker = "%s"\n' "$RUST_TARGET" "$MUSL_CC" >> .cargo/config.toml

# Environment for vendored OpenSSL build with musl
ENV OPENSSL_NO_VENDOR=0
# Ensure fully static binary (no dynamic interpreter needed)
ENV RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=static"

COPY --from=planner /app/recipe.json recipe.json

# Cook dependencies with target
RUN RUST_TARGET=$(cat /tmp/rust_target) && \
    MUSL_CC=$(cat /tmp/musl_cc) && \
    CC=$MUSL_CC cargo chef cook --release --recipe-path recipe.json --target $RUST_TARGET

COPY . .

# Build the application
RUN RUST_TARGET=$(cat /tmp/rust_target) && \
    MUSL_CC=$(cat /tmp/musl_cc) && \
    CC=$MUSL_CC GIT_VERSION=${GIT_VERSION} \
    cargo build --release --target $RUST_TARGET && \
    cp target/$RUST_TARGET/release/rdrs /app/rdrs

# Stage 4: Runtime - scratch since binary is fully static
FROM scratch

COPY --from=builder /app/rdrs /rdrs

VOLUME /data

ENV DATABASE_URL=/data/rdrs.sqlite3
ENV SERVER_PORT=3000

EXPOSE 3000

ENTRYPOINT ["/rdrs"]
