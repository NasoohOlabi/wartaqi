FROM rust:1.93-bookworm AS build
WORKDIR /app/rust-telegram
COPY rust-common /app/rust-common
COPY rust-telegram/Cargo.toml rust-telegram/Cargo.lock ./
COPY rust-telegram/src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /app/rust-telegram/target/release/wartaq-telegram /usr/local/bin/wartaq-telegram
CMD ["wartaq-telegram"]
