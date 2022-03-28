FROM docker.io/curlimages/curl:latest as linkerd
ARG LINKERD_AWAIT_VERSION=v0.2.5
RUN curl -sSLo /tmp/linkerd-await https://github.com/linkerd/linkerd-await/releases/download/release%2F${LINKERD_AWAIT_VERSION}/linkerd-await-${LINKERD_AWAIT_VERSION}-amd64 && \
    chmod 755 /tmp/linkerd-await
