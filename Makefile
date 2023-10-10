MAKEFILE_PATH := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
RUNTIME_PATH ?= "/usr/bin/runc"
PROTO_PATH ?= "conmon-rs/common/proto"
BINARY := conmonrs
BUILD_DIR ?= .build
GOTOOLS_GOPATH ?= $(BUILD_DIR)/gotools
GOTOOLS_BINDIR ?= $(GOTOOLS_GOPATH)/bin
GINKGO_FLAGS ?= -vv --trace --race --randomize-all --flake-attempts 3 --show-node-events --timeout 5m -r pkg/client
TEST_FLAGS ?=
PACKAGE_NAME ?= $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[2] | [ .name, .version ] | join("-v")')
PREFIX ?= /usr
CI_TAG ?=
GOLANGCI_LINT_VERSION := v1.54.1
ZEITGEIST_VERSION := v0.4.1
GINKGO_VERSION := v2.12.0

default:
	cargo build

release:
	cargo build --release

.PHONY: release-static
release-static:
	RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target x86_64-unknown-linux-gnu
	strip -s target/x86_64-unknown-linux-gnu/release/conmonrs
	ldd target/x86_64-unknown-linux-gnu/release/conmonrs 2>&1 | grep -qE '(statically linked)|(not a dynamic executable)'

lint: lint-rust lint-go

lint-rust:
	cargo fmt && git diff --exit-code
	cargo clippy --all-targets --all-features -- -D warnings

lint-go: .install.golangci-lint
	$(GOTOOLS_BINDIR)/golangci-lint version
	$(GOTOOLS_BINDIR)/golangci-lint linters
	GL_DEBUG=gocritic $(GOTOOLS_BINDIR)/golangci-lint run

.PHONY: verify-dependencies
verify-dependencies: $(GOTOOLS_BINDIR)/zeitgeist
	$(GOTOOLS_BINDIR)/zeitgeist validate --local-only --base-path . --config dependencies.yaml

unit:
	cargo test --no-fail-fast

integration: .install.ginkgo release # It needs to be release so we correctly test the RSS usage
	export CONMON_BINARY="$(MAKEFILE_PATH)target/release/$(BINARY)" && \
	export RUNTIME_BINARY="$(RUNTIME_PATH)" && \
	export MAX_RSS_KB=10240 && \
	sudo -E "$(GOTOOLS_BINDIR)/ginkgo" $(TEST_FLAGS) $(GINKGO_FLAGS)

integration-static: .install.ginkgo # It needs to be release so we correctly test the RSS usage
	export CONMON_BINARY="$(MAKEFILE_PATH)target/x86_64-unknown-linux-gnu/release/$(BINARY)" && \
	if [ ! -f "$$CONMON_BINARY" ]; then \
		$(MAKE) release-static; \
	fi && \
	export RUNTIME_BINARY="$(RUNTIME_PATH)" && \
	export MAX_RSS_KB=7500 && \
	sudo -E "$(GOTOOLS_BINDIR)/ginkgo" $(TEST_FLAGS) $(GINKGO_FLAGS)

.install.ginkgo:
	GOBIN=$(abspath $(GOTOOLS_BINDIR)) go install github.com/onsi/ginkgo/v2/ginkgo@$(GINKGO_VERSION)

.install.golangci-lint:
	curl -sSfL https://raw.githubusercontent.com/golangci/golangci-lint/master/install.sh | \
		BINDIR=$(abspath $(GOTOOLS_BINDIR)) sh -s $(GOLANGCI_LINT_VERSION)

$(GOTOOLS_BINDIR)/zeitgeist:
	mkdir -p $(GOTOOLS_BINDIR)
	curl -sSfL -o $(GOTOOLS_BINDIR)/zeitgeist \
		https://github.com/kubernetes-sigs/zeitgeist/releases/download/$(ZEITGEIST_VERSION)/zeitgeist_$(ZEITGEIST_VERSION:v%=%)_linux_amd64
	chmod +x $(GOTOOLS_BINDIR)/zeitgeist

clean:
	rm -rf target/

.INTERMEDIATE: internal/proto/conmon.capnp
internal/proto/conmon.capnp:
	cat $(PROTO_PATH)/conmon.capnp $(PROTO_PATH)/go-patch > $@

update-proto: internal/proto/conmon.capnp
	$(eval GO_CAPNP_VERSION ?= $(shell grep '^\s*capnproto.org/go/capnp/v3 v3\.' go.mod | grep -o 'v3\..*'))
	go install capnproto.org/go/capnp/v3/capnpc-go@$(GO_CAPNP_VERSION)
	capnp compile \
		-I$(shell go env GOMODCACHE)/capnproto.org/go/capnp/v3@$(GO_CAPNP_VERSION)/std \
		-ogo internal/proto/conmon.capnp

.PHONY: lint lint-go lint-rust clean unit integration update-proto

.PHONY: create-release-packages
create-release-packages: release
	if [ "$(PACKAGE_NAME)" != "conmonrs-$(CI_TAG)" ]; then \
		echo "crate version and tag mismatch" ; \
		exit 1 ; \
	fi
	cargo vendor -q && tar zcf $(PACKAGE_NAME)-vendor.tar.gz vendor && rm -rf vendor
	git archive --format tar --prefix=conmonrs-$(CI_TAG)/ $(CI_TAG) | gzip >$(PACKAGE_NAME).tar.gz


.PHONY: install
install:
	mkdir -p "${DESTDIR}$(PREFIX)/bin"
	install -D -t "${DESTDIR}$(PREFIX)/bin" target/release/conmonrs

# Only meant to build the latest HEAD commit + any uncommitted changes
# Not a replacement for the distro package
.PHONY: rpm
rpm:
	rpkg local

nixpkgs:
	@nix run -f channel:nixpkgs-unstable nix-prefetch-git -- \
		--no-deepClone https://github.com/nixos/nixpkgs > nix/nixpkgs.json
