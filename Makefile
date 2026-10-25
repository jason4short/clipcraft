# clipcraft: build, check, and install.
#
# `make install` builds clipcraft.app into ~/Applications (APPLICATIONS=
# /Applications for everyone) and links the binary into BINDIR so it still
# runs from a shell.

PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
APPLICATIONS ?= $(HOME)/Applications

MANIFEST = Cargo.toml
CARGO ?= cargo
TARGET = target/release/clipcraft
ICON = assets/clipcraft.svg
PLIST = packaging/Info.plist.in
APP = target/clipcraft.app
APP_TMP = target/app-build

.PHONY: help build run install uninstall app lint test ci fmt clean

help:
	@echo "clipcraft"
	@echo
	@echo "  make build       release build"
	@echo "  make run         build and run"
	@echo "  make app         build target/clipcraft.app"
	@echo "  make install     install the app into $(APPLICATIONS)"
	@echo "  make uninstall   remove what install put there"
	@echo "  make lint        rustfmt --check and clippy -D warnings"
	@echo "  make test        core and window-harness tests"
	@echo "  make ci          lint, then test"
	@echo "  make fmt         format in place"
	@echo "  make clean       cargo clean"

# Always ask cargo: it is incremental and knows every source file, where a
# make file-target would only compare the binary against the manifest and
# happily install a stale build.
build:
	$(CARGO) build --release

run: build
	$(TARGET)

lint:
	$(CARGO) xtask lint

test:
	$(CARGO) xtask test

ci: lint test

fmt:
	$(CARGO) xtask fmt-fix

install: app
	install -d "$(APPLICATIONS)" "$(BINDIR)"
	rm -rf "$(APPLICATIONS)/clipcraft.app"
	ditto "$(APP)" "$(APPLICATIONS)/clipcraft.app"
	ln -sf "$(APPLICATIONS)/clipcraft.app/Contents/MacOS/clipcraft" \
	    "$(BINDIR)/clipcraft"
	@echo
	@echo "installed:"
	@echo "  $(APPLICATIONS)/clipcraft.app"
	@echo "  $(BINDIR)/clipcraft -> the app's binary"

uninstall:
	rm -rf "$(APPLICATIONS)/clipcraft.app"
	rm -f "$(BINDIR)/clipcraft"
	@echo "removed"

# A self-contained bundle. Quick Look renders an SVG at its intrinsic size,
# so the icon is re-sized to 1024 before rasterising, or the mark would land
# in the corner of a blank canvas.
app: build
	rm -rf $(APP) $(APP_TMP)
	mkdir -p $(APP)/Contents/MacOS $(APP)/Contents/Resources \
	    $(APP_TMP)/clipcraft.iconset
	install -m755 $(TARGET) $(APP)/Contents/MacOS/clipcraft
	sed 's/width="64" height="64"/width="1024" height="1024"/' $(ICON) \
	    > $(APP_TMP)/icon.svg
	qlmanage -t -s 1024 -o $(APP_TMP) $(APP_TMP)/icon.svg >/dev/null 2>&1
	set -e; for s in 16 32 128 256 512; do \
	    sips -z $$s $$s $(APP_TMP)/icon.svg.png \
	        --out $(APP_TMP)/clipcraft.iconset/icon_$${s}x$${s}.png \
	        >/dev/null; \
	    d=$$((s * 2)); \
	    sips -z $$d $$d $(APP_TMP)/icon.svg.png \
	        --out $(APP_TMP)/clipcraft.iconset/icon_$${s}x$${s}@2x.png \
	        >/dev/null; \
	done
	iconutil -c icns $(APP_TMP)/clipcraft.iconset \
	    -o $(APP)/Contents/Resources/clipcraft.icns
	VERSION=$$(sed -n 's/^version = "\(.*\)"/\1/p' $(MANIFEST) | head -1) && \
	sed -e "s|@VERSION@|$$VERSION|" $(PLIST) > $(APP)/Contents/Info.plist
	codesign --force --sign - "$(APP)"
	rm -rf $(APP_TMP)
	@echo "built $(APP)"

clean:
	$(CARGO) clean
