FROM rust:1.93-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked
FROM debian:bookworm-slim
COPY --from=build /app/target/release/wartaq-workspace-relay /usr/local/bin/relay
USER 65532:65532
EXPOSE 3001
CMD ["relay"]
