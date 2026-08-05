FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 leddy
COPY --from=build /src/target/release/leddy-api-server /usr/local/bin/leddy-api-server
USER 10001
EXPOSE 8080
ENTRYPOINT ["leddy-api-server"]
