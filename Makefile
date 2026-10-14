# Release builds, and the check that says whether one is shippable.
#
# Everyday work is plain `cargo build` / `cargo test` — this file is only for
# producing a binary that runs on someone else's machine. See `Containerfile`
# for why that needs a container at all.

CONTAINER ?= podman
RUST_IMAGE ?= docker.io/library/rust:bookworm
IMAGE      ?= lightforge-build
OUT        ?= target/container/release
DIST       ?= target/dist

# For `make install` only; the packages set their own roots.
PREFIX     ?= /usr/local
DESTDIR    ?=

# Read from the workspace manifest rather than written twice. `PKGVER` is the
# same version in the form `dpkg` and `rpm` both order correctly: a `~` sorts
# *before* the release it precedes, where `-` does not, so `1.0.0-beta.1`
# would upgrade to nothing and `1.0.0~beta.1` upgrades to `1.0.0`.
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
PKGVER  := $(subst -,~,$(VERSION))
TARBALL := lightforge-$(VERSION)-x86_64-linux.tar.gz

# The highest glibc a release may ask for. Debian 12 is 2.36; anything above
# that means the build escaped the container and picked up the host's libc.
GLIBC_MAX ?= 2.36

BINARIES := lightforge lightforge-gui

.PHONY: release image check-glibc clean-release cli-doc check-cli-doc \
	packages deb rpm tarball checksums github-names check-binaries \
	clean-packages install uninstall

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

## Install a source build the way a package would.
##
## For someone building from the tree: `cargo build --release` leaves two
## binaries and nothing else, so without this there is no menu entry and no
## icon. `PREFIX=$$HOME/.local make install` needs no root.
##
## The three packages do **not** call this — each of them lists the same files
## in its own idiom, because that is what their tooling reads.
install:
	install -Dm755 target/release/lightforge-gui $(DESTDIR)$(PREFIX)/bin/lightforge-gui
	install -Dm755 target/release/lightforge     $(DESTDIR)$(PREFIX)/bin/lightforge
	install -Dm644 packaging/pro.tuxcore.Lightforge.desktop \
	  $(DESTDIR)$(PREFIX)/share/applications/pro.tuxcore.Lightforge.desktop
	install -Dm644 packaging/pro.tuxcore.Lightforge.metainfo.xml \
	  $(DESTDIR)$(PREFIX)/share/metainfo/pro.tuxcore.Lightforge.metainfo.xml
	@for size in 16 22 24 32 48 64 128 256 512; do \
	  install -Dm644 "packaging/icons/hicolor/$${size}x$${size}/apps/pro.tuxcore.Lightforge.png" \
	    "$(DESTDIR)$(PREFIX)/share/icons/hicolor/$${size}x$${size}/apps/pro.tuxcore.Lightforge.png"; \
	done
	install -Dm644 packaging/icons/hicolor/scalable/apps/pro.tuxcore.Lightforge.svg \
	  $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/pro.tuxcore.Lightforge.svg
	install -Dm644 LICENSE $(DESTDIR)$(PREFIX)/share/licenses/lightforge/LICENSE
	@echo "installed to $(DESTDIR)$(PREFIX)"

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/lightforge-gui \
	      $(DESTDIR)$(PREFIX)/bin/lightforge \
	      $(DESTDIR)$(PREFIX)/share/applications/pro.tuxcore.Lightforge.desktop \
	      $(DESTDIR)$(PREFIX)/share/metainfo/pro.tuxcore.Lightforge.metainfo.xml \
	      $(DESTDIR)$(PREFIX)/share/icons/hicolor/*/apps/pro.tuxcore.Lightforge.png \
	      $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/pro.tuxcore.Lightforge.svg
	rm -rf $(DESTDIR)$(PREFIX)/share/licenses/lightforge
	@echo "removed from $(DESTDIR)$(PREFIX)"

## Everything a release page carries, from one `make release`.
##
## The three package targets deliberately do **not** depend on `release`:
## rebuilding 27 MB of GUI three times to produce three files from the same
## two binaries is a minute nobody needs to spend. Run `make release` first —
## each target checks the binaries are there.
packages: tarball deb rpm checksums
	@echo
	@echo "Release artifacts in $(DIST):"
	@ls -lh $(DIST) | awk 'NR>1 {print "  " $$9 "  " $$5}'

## The portable tarball: what the `lightforge-bin` PKGBUILD installs, and what
## anyone without a package manager we support downloads.
##
## It carries the desktop entry, the icons and the AppStream file beside the
## binaries, so the PKGBUILD installs from this one source and the Arch
## package has the same contents as the deb and the rpm.
tarball: check-binaries
	@mkdir -p $(DIST)
	@rm -rf target/tarball/lightforge-$(VERSION)
	@mkdir -p target/tarball/lightforge-$(VERSION)
	@cp $(addprefix $(OUT)/,$(BINARIES)) target/tarball/lightforge-$(VERSION)/
	@cp -r packaging/icons packaging/pro.tuxcore.Lightforge.desktop \
	       packaging/pro.tuxcore.Lightforge.metainfo.xml \
	       LICENSE README.md RELEASES.md TROUBLESHOOTING.md docs/CLI.md \
	       target/tarball/lightforge-$(VERSION)/
	@tar -czf $(DIST)/$(TARBALL) -C target/tarball lightforge-$(VERSION)
	@echo "ok   $(DIST)/$(TARBALL)"

## A .deb, written natively — no `dpkg` on this machine.
##
## `CARGO_TARGET_DIR` is what points the asset paths in the manifest at the
## container build; `--no-build` and `--no-strip` because `release` already
## did both.
deb: check-binaries
	@mkdir -p $(DIST)
	CARGO_TARGET_DIR=target/container cargo deb \
	  --package lightforge-gui \
	  --no-build --no-strip \
	  --deb-version '$(PKGVER)-1' \
	  --output $(DIST)/

## An .rpm, written natively — no `rpmbuild` on this machine either.
rpm: check-binaries
	@mkdir -p $(DIST)
	cargo generate-rpm \
	  --package crates/launcher-gui \
	  --target-dir target/container \
	  --set-metadata 'version = "$(PKGVER)"' \
	  --set-metadata 'release = "1"' \
	  --output $(DIST)/

## Rename the packages to the names GitHub will actually serve them under.
##
## A release asset may not have a `~` in its name: GitHub rewrites it to `.`,
## so `lightforge_1.0.0~beta.1-1_amd64.deb` is downloaded as
## `lightforge_1.0.0.beta.1-1_amd64.deb` — and a `SHA256SUMS` listing the name
## we built fails `sha256sum -c` for everyone. Renaming here means what is
## built is exactly what is published. Only the filename changes; the version
## *inside* the package is still `1.0.0~beta.1`, which is what `apt` and `dnf`
## read and order by.
github-names:
	@for f in $(DIST)/*~*; do \
	  [ -e "$$f" ] || continue; \
	  mv "$$f" "$$(echo "$$f" | tr '~' '.')"; \
	done

## The sums the AUR PKGBUILDs and the release page quote.
checksums: github-names
	@cd $(DIST) && rm -f SHA256SUMS && sha256sum lightforge* > SHA256SUMS
	@cat $(DIST)/SHA256SUMS

## Fail with a useful sentence rather than a missing-file error from tar,
## and refuse binaries built from another version: packages are named from
## Cargo.toml, so stale binaries would ship under the new name (beta.2 nearly
## did). The timeout is for a GUI built before it answered `--version`,
## which would open a window instead and hold the build.
check-binaries:
	@for bin in $(addprefix $(OUT)/,$(BINARIES)); do \
	  [ -f "$$bin" ] || { echo "missing $$bin — run \`make release\` first"; exit 1; }; \
	  got=$$(timeout 10 "$$bin" --version 2>/dev/null); \
	  want="$$(basename "$$bin") $(VERSION)"; \
	  [ "$$got" = "$$want" ] || { echo "$$bin reports '$$got', expected '$$want' — run \`make release\`"; exit 1; }; \
	done
	@echo "ok   binaries are $(VERSION)"

clean-packages:
	rm -rf $(DIST) target/tarball target/debian target/container/generate-rpm

clean-release:
	rm -rf target/container
