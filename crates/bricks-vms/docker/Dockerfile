# cersei/sandbox-base — minimal base image for Bricks sandboxes.
#
# Build:
#   cargo build --release --bin cersei-envd \
#       --target x86_64-unknown-linux-musl
#   docker build -t cersei/sandbox-base:latest \
#       --build-arg ENVD_BIN=target/x86_64-unknown-linux-musl/release/cersei-envd \
#       crates/bricks-vms/docker/
#
# At runtime the host bind-mounts a Unix socket into /run/cersei-envd.sock
# and the daemon listens there.

FROM alpine:3.20

ARG ENVD_BIN=cersei-envd

RUN apk add --no-cache \
        bash \
        ca-certificates \
        coreutils \
        findutils \
        git \
        curl

WORKDIR /work

COPY ${ENVD_BIN} /usr/local/bin/cersei-envd
RUN chmod +x /usr/local/bin/cersei-envd

ENV CERSEI_ENVD_SOCKET=/run/cersei-envd.sock

# By default, agents drive the sandbox via `docker exec`; envd is launched
# only when the host requests it. Override CMD if you want envd up at boot.
CMD ["tail", "-f", "/dev/null"]
