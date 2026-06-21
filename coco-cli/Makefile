# Quick-release helpers for @coco-rs/coco-cli.
# Run from the coco-cli/ directory.
#
# Usage:
#   make build [TARGET=<triple>]    Build coco for one target into vendor/
#   make smoke                       Run the launcher against the built binary
#   make pack VERSION=0.1.0          Pack meta + every platform with vendor/
#   make tag VERSION=0.1.0           git tag coco-vVERSION + push (triggers CI)
#   make publish VERSION=0.1.0       npm publish all tarballs in correct order
#   make clean                       Remove vendor/ and dist/

VERSION ?= 0.1.0
TARGET  ?= $(shell rustc -vV 2>/dev/null | sed -n 's/^host: //p')

REPO_ROOT := $(abspath ..)
COCO_RS   := $(REPO_ROOT)/coco-rs
DIST_DIR  := $(REPO_ROOT)/dist/npm
SCRIPTS   := scripts
VENDOR    := vendor

# Map target triple → npm platform tag.
TAG_x86_64-unknown-linux-musl  := linux-x64
TAG_aarch64-unknown-linux-musl := linux-arm64
TAG_aarch64-apple-darwin       := darwin-arm64

PLATFORM_TAG := $(TAG_$(TARGET))

.PHONY: help
help:
	@awk '/^[^#]/{exit} NR>1{sub(/^# ?/, ""); print}' Makefile

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

.PHONY: build
build:
ifeq ($(PLATFORM_TAG),)
	$(error TARGET=$(TARGET) is not a supported triple. Use one of: \
	  x86_64-unknown-linux-musl, aarch64-unknown-linux-musl, aarch64-apple-darwin)
endif
	cd $(COCO_RS) && cargo build --release --target $(TARGET) -p coco-cli --bin coco --features jemalloc
	mkdir -p $(VENDOR)/$(TARGET)/coco
	cp $(COCO_RS)/target/$(TARGET)/release/coco $(VENDOR)/$(TARGET)/coco/coco
	chmod +x $(VENDOR)/$(TARGET)/coco/coco
	@echo "→ $(VENDOR)/$(TARGET)/coco/coco"

.PHONY: smoke
smoke:
	node bin/coco-cli.js --version

# ---------------------------------------------------------------------------
# Pack — produces tarballs in $(DIST_DIR)
# ---------------------------------------------------------------------------

$(DIST_DIR):
	mkdir -p $@

.PHONY: pack-meta
pack-meta: | $(DIST_DIR)
	python3 $(SCRIPTS)/build_npm_package.py --package coco-cli --version $(VERSION) \
	  --pack-output $(DIST_DIR)/coco-cli-npm-$(VERSION).tgz

.PHONY: pack-linux-x64 pack-linux-arm64 pack-darwin-arm64
pack-linux-x64: | $(DIST_DIR)
	python3 $(SCRIPTS)/build_npm_package.py --package coco-cli-linux-x64 \
	  --version $(VERSION) --vendor-src $(VENDOR) \
	  --pack-output $(DIST_DIR)/coco-cli-npm-linux-x64-$(VERSION).tgz
pack-linux-arm64: | $(DIST_DIR)
	python3 $(SCRIPTS)/build_npm_package.py --package coco-cli-linux-arm64 \
	  --version $(VERSION) --vendor-src $(VENDOR) \
	  --pack-output $(DIST_DIR)/coco-cli-npm-linux-arm64-$(VERSION).tgz
pack-darwin-arm64: | $(DIST_DIR)
	python3 $(SCRIPTS)/build_npm_package.py --package coco-cli-darwin-arm64 \
	  --version $(VERSION) --vendor-src $(VENDOR) \
	  --pack-output $(DIST_DIR)/coco-cli-npm-darwin-arm64-$(VERSION).tgz

# Pack the meta package + every platform whose vendor/<triple>/coco/coco exists.
.PHONY: pack
pack: pack-meta
	@if [ -f $(VENDOR)/x86_64-unknown-linux-musl/coco/coco  ]; then $(MAKE) pack-linux-x64; fi
	@if [ -f $(VENDOR)/aarch64-unknown-linux-musl/coco/coco ]; then $(MAKE) pack-linux-arm64; fi
	@if [ -f $(VENDOR)/aarch64-apple-darwin/coco/coco       ]; then $(MAKE) pack-darwin-arm64; fi
	@echo ""
	@echo "Tarballs in $(DIST_DIR):"
	@ls -1 $(DIST_DIR)

# ---------------------------------------------------------------------------
# Release
# ---------------------------------------------------------------------------

.PHONY: tag
tag:
	git tag -a coco-v$(VERSION) -m "Release $(VERSION)"
	git push origin coco-v$(VERSION)

# Publish in npm's required order: platform packages first, meta last.
# Skips any tarball that doesn't exist (so partial releases work).
.PHONY: publish
publish:
	@for f in $(DIST_DIR)/coco-cli-npm-linux-x64-$(VERSION).tgz \
	          $(DIST_DIR)/coco-cli-npm-linux-arm64-$(VERSION).tgz \
	          $(DIST_DIR)/coco-cli-npm-darwin-arm64-$(VERSION).tgz; do \
	  if [ -f "$$f" ]; then \
	    echo "+ npm publish $$f"; \
	    npm publish "$$f" --access public; \
	  fi; \
	done
	@meta=$(DIST_DIR)/coco-cli-npm-$(VERSION).tgz; \
	  if [ -f "$$meta" ]; then \
	    echo "+ npm publish $$meta"; \
	    npm publish "$$meta" --access public; \
	  else \
	    echo "ERROR: meta tarball $$meta not found — run 'make pack VERSION=$(VERSION)' first" >&2; \
	    exit 1; \
	  fi

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------

.PHONY: clean
clean:
	rm -rf $(VENDOR) $(DIST_DIR)
