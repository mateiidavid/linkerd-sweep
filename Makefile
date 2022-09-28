TAG ?= test
CLUSTER_NAME ?= dev

all: build import

.PHONY: build
build: ## build image
	DOCKER_BUILDKIT=1 docker build --progress=plain -t ghcr.io/mateiidavid/linkerd-sweep:$(TAG) .
.PHONY: import
import: ## imports image
	k3d image import --cluster $(CLUSTER_NAME) ghcr.io/mateiidavid/linkerd-sweep:$(TAG)
