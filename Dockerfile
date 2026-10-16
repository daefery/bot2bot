FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* && useradd --create-home bot
WORKDIR /app
COPY --from=build /src/target/release/bot2bot /usr/local/bin/bot2bot
COPY profile ./profile
USER bot
# The platform sets PORT; BOT2BOT_SERVER_KEY and the BOT2BOT_MODEL_* values are secrets.
CMD ["bot2bot", "serve"]
