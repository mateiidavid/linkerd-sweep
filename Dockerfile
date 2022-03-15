ARG RUST_VERSION=1.58.1
ARG RUNTIME_IMAGE=gcr.io/distroless/cc

# Builds the operator binary.
FROM rust:$RUST_IMAGE as build
ARG TARGETARCH
WORKDIR /build
COPY Cargo.toml Cargo.lock . /build/
RUN --mount=type=cache,target=target \
    --mount=type=cache,from=rust:1.56.1,source=/usr/local/cargo,target=/usr/local/cargo \
    cargo build --locked --target=x86_64-unknown-linux-gnu --release --package=linkerd-sweep && \
    mv target/x86_64-unknown-linux-gnu/release/linkerd-sweep /tmp/

FROM $RUNTIME_IMAGE
COPY --from=build /tmp/linkerd-sweep /bin/
ENTRYPOINT ["/bin/linkerd-sweep"]
