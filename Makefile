TAG ?= latest
CLUSTER_NAME ?= dev

all: build import

.PHONY: build
build: ## build image
	DOCKER_BUILDKIT=1 docker build -t linkerd-sweep:$(TAG) .
.PHONY: import
import: ## imports image
	k3d image import --cluster $(CLUSTER_NAME) linkerd-sweep:$(TAG)
