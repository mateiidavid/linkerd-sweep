##
## Rust
## 

FROM docker.io/library/rust:1.63.0 as rust
WORKDIR /build
COPY Cargo.toml Cargo.lock .
COPY src /build/src
ARG TARGETARCH
RUN --mount=type=cache,target=target \
   --mount=type=cache,from=docker.io/library/rust:1.63.0-slim,source=/usr/local/cargo,target=/usr/local/cargo \
   cargo fetch
RUN --mount=type=cache,target=target \
   --mount=type=cache,from=docker.io/library/rust:1.63.0-slim,source=/usr/local/cargo,target=/usr/local/cargo \
   cargo build --locked --target=x86_64-unknown-linux-gnu --release && \
   mv "target/x86_64-unknown-linux-gnu/release/linkerd-sweep" /tmp/

##
## Runtime
## 

FROM gcr.io/distroless/cc as runtime
COPY --from=rust /tmp/linkerd-sweep /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/linkerd-sweep"]

