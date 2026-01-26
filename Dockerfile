# Stage 1: Chef - prepare recipe
FROM rust:1.92-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

# Stage 2: Planner - create recipe.json
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder - build dependencies then app
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

# Create data directory with placeholder for copying to runtime
RUN mkdir -p /data && touch /data/.keep

# Stage 4: Runtime
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/rdrs /rdrs

# Create /data directory with world-writable permissions (777)
# This allows the container to run with any user (e.g., user: 1000:1000 in docker-compose)
# The directory needs to be writable for SQLite to create journal/WAL files
COPY --from=builder --chmod=777 /data /data

VOLUME /data

ENV DATABASE_URL=/data/rdrs.sqlite3
ENV SERVER_PORT=3000

EXPOSE 3000

ENTRYPOINT ["/rdrs"]
