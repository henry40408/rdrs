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

# Stage 4: Runtime
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/rdrs /rdrs

VOLUME /data

ENV DATABASE_URL=/data/rdrs.sqlite3
ENV SERVER_PORT=3000

EXPOSE 3000

ENTRYPOINT ["/rdrs"]
