# Release builds, and the check that says whether one is shippable.
#
# Everyday work is plain `cargo build` / `cargo test` — this file is only for
# producing a binary that runs on someone else's machine. See `Containerfile`
# for why that needs a container at all.

CONTAINER ?= podman
RUST_IMAGE ?= docker.io/library/rust:bookworm
IMAGE      ?= lightforge-build
OUT        ?= target/container/release

# The highest glibc a release may ask for. Debian 12 is 2.36; anything above
# that means the build escaped the container and picked up the host's libc.
GLIBC_MAX ?= 2.36

BINARIES := lightforge lightforge-gui

.PHONY: release image check-glibc clean-release cli-doc check-cli-doc

## Build release binaries against an old glibc, and verify they are portable.
release: image
	$(CONTAINER) run --rm \
	  --volume "$(CURDIR):/src:Z" \
	  --workdir /src \
	  $(IMAGE) \
	  cargo build --release --locked
	strip $(addprefix $(OUT)/,$(BINARIES))
	@$(MAKE) --no-print-directory check-glibc
	@echo
	@echo "Release binaries in $(OUT):"
	@ls -lh $(addprefix $(OUT)/,$(BINARIES)) | awk '{print "  " $$9 "  " $$5}'

image:
	$(CONTAINER) build --build-arg RUST_IMAGE=$(RUST_IMAGE) --tag $(IMAGE) --file Containerfile .

## Fail if a binary demands a glibc newer than the floor.
##
## The whole point of the container, asserted rather than assumed — a release
## that quietly regains the host's glibc looks fine here and fails on a Deck.
check-glibc:
	@fail=0; \
	for bin in $(addprefix $(OUT)/,$(BINARIES)); do \
	  if [ ! -f "$$bin" ]; then echo "missing: $$bin"; fail=1; continue; fi; \
	  needs=$$(objdump -T "$$bin" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1); \
	  highest=$${needs#GLIBC_}; \
	  top=$$(printf '%s\n%s\n' "$(GLIBC_MAX)" "$$highest" | sort -V | tail -1); \
	  if [ "$$top" = "$(GLIBC_MAX)" ]; then \
	    echo "ok   $$bin needs glibc $$highest (floor $(GLIBC_MAX))"; \
	  else \
	    echo "FAIL $$bin needs glibc $$highest, above the $(GLIBC_MAX) floor"; \
	    fail=1; \
	  fi; \
	done; \
	exit $$fail

## Regenerate docs/CLI.md from the binary's own --help.
##
## The page is forty-four commands long; typing it by hand guarantees it is
## wrong the first time a flag changes, and nobody finds out until a reader
## follows it.
cli-doc:
	cargo build --release --bin lightforge
	sh docs/gen-cli.sh

## Fail if docs/CLI.md is not what the current binary would produce.
##
## Belongs in whatever runs before a release: the generator is only useful if
## somebody notices it was not run.
check-cli-doc: cli-doc
	@git diff --quiet -- docs/CLI.md \
	  || { echo "docs/CLI.md is out of date — commit the regenerated page"; exit 1; }
	@echo "ok   docs/CLI.md matches the binary"

clean-release:
	rm -rf target/container
