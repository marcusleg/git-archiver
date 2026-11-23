# syntax=docker/dockerfile:1

# Builds a static musl binary, so the same file runs in the image below and on
# any Linux host (it's also what the release attaches).
FROM docker.io/library/rust:1.100-alpine AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin git-archiver \
    && cp target/release/git-archiver /git-archiver

# Just the binary, for `docker buildx build --target binary --output ...`.
FROM scratch AS binary
COPY --from=build /git-archiver /git-archiver

FROM docker.io/library/alpine:3.24
# git-daemon carries `git http-backend`, which serves the archives.
RUN apk add --no-cache git git-daemon ca-certificates \
    && adduser -D -H -u 10001 -s /sbin/nologin archiver \
    && mkdir /data \
    && chown archiver:archiver /data
COPY --from=build /git-archiver /usr/local/bin/git-archiver
ENV GIT_ARCHIVER_DATA_DIR=/data
VOLUME /data
EXPOSE 8080
USER archiver
ENTRYPOINT ["/usr/local/bin/git-archiver"]
