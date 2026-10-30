# Build the VPN backend in router/ and the NusaProtect GUI.
#   make backend    gpclient, gpservice, gpauth, allowroute
#   make frontend   NusaProtect window (Vite assets + Tauri binary, no installer)
#   make all        backend, then frontend
#   make install    build both from this tree and install under PREFIX (no download)

CARGO ?= cargo
PNPM ?= pnpm
ROUTER_MANIFEST := router/Cargo.toml
VERSION := $(shell tr -d '[:space:]' < VERSION)
PREFIX ?= /usr
DESTDIR ?=
ARCH := $(shell uname -m)
RELEASE_ROUTER := router/target/release
RELEASE_GUI := src-tauri/target/release/nusaprotect
BACKEND_STAGE := .build/nusaprotect-backend
BACKEND_TARBALL := .build/nusaprotect-backend-linux-$(ARCH).tar.gz

ifeq ($(DESTDIR),)
  SUDO := $(shell if [ "$$(id -u)" -eq 0 ]; then echo; else echo sudo; fi)
else
  SUDO :=
endif

.DEFAULT_GOAL := help

.PHONY: help check-version backend frontend all backend-dist install

help:
	@printf '%s\n' \
		'make check-version Verify VERSION $(VERSION) has a changelog section' \
		'make backend       Build gpclient, gpservice, gpauth, and allowroute (release)' \
		'make frontend      Build the NusaProtect GUI (release, no installer bundle)' \
		'make all           Build the backend, then the frontend' \
		'make install       Build both from this tree and install under $(PREFIX)' \
		'make backend-dist  Pack the backend binaries into $(BACKEND_TARBALL)'

check-version:
	@set -eu; \
	version="$(VERSION)"; \
	if ! printf '%s\n' "$$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$$'; then \
		echo "VERSION is not major.minor.patch: $$version" >&2; \
		exit 1; \
	fi; \
	if ! grep -qE "^## \\[$$version\\]( |$$)" CHANGELOG.md; then \
		echo "CHANGELOG.md has no section for VERSION=$$version" >&2; \
		exit 1; \
	fi

backend:
	$(CARGO) build --manifest-path $(ROUTER_MANIFEST) --release \
		-p gpclient -p gpservice -p gpauth -p allowroute

frontend:
	$(PNPM) install --frozen-lockfile
	$(PNPM) tauri build --no-bundle

all: backend frontend

backend-dist: backend
	rm -rf $(BACKEND_STAGE)
	install -d $(BACKEND_STAGE)/bin $(BACKEND_STAGE)/libexec/npclient
	install -m 755 $(RELEASE_ROUTER)/gpclient $(BACKEND_STAGE)/bin/npclient
	install -m 755 $(RELEASE_ROUTER)/gpservice $(BACKEND_STAGE)/bin/npservice
	install -m 755 $(RELEASE_ROUTER)/gpauth $(BACKEND_STAGE)/bin/npauth
	install -m 755 $(RELEASE_ROUTER)/allowroute $(BACKEND_STAGE)/libexec/npclient/allowroute
	install -m 755 $(RELEASE_ROUTER)/gp-vpnc-script-installer $(BACKEND_STAGE)/libexec/npclient/np-vpnc-script-installer
	install -m 755 $(RELEASE_ROUTER)/gp-hip-script-installer $(BACKEND_STAGE)/libexec/npclient/np-hip-script-installer
	install -m 755 router/packaging/files/usr/libexec/npclient/vpnc-script $(BACKEND_STAGE)/libexec/npclient/vpnc-script
	tar -C $(BACKEND_STAGE) -czf $(BACKEND_TARBALL) bin libexec

install: all
	$(SUDO) install -d $(DESTDIR)$(PREFIX)/bin $(DESTDIR)$(PREFIX)/libexec/npclient \
		$(DESTDIR)$(PREFIX)/share/applications $(DESTDIR)$(PREFIX)/share/polkit-1/actions \
		$(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps \
		$(DESTDIR)$(PREFIX)/lib/NetworkManager/dispatcher.d/pre-down.d
	$(SUDO) install -m 755 $(RELEASE_ROUTER)/gpclient $(DESTDIR)$(PREFIX)/bin/npclient
	$(SUDO) install -m 755 $(RELEASE_ROUTER)/gpservice $(DESTDIR)$(PREFIX)/bin/npservice
	$(SUDO) install -m 755 $(RELEASE_ROUTER)/gpauth $(DESTDIR)$(PREFIX)/bin/npauth
	$(SUDO) install -m 755 $(RELEASE_GUI) $(DESTDIR)$(PREFIX)/bin/npgui
	$(SUDO) install -m 755 $(RELEASE_ROUTER)/allowroute $(DESTDIR)$(PREFIX)/libexec/npclient/allowroute
	$(SUDO) install -m 755 $(RELEASE_ROUTER)/gp-vpnc-script-installer $(DESTDIR)$(PREFIX)/libexec/npclient/np-vpnc-script-installer
	$(SUDO) install -m 755 $(RELEASE_ROUTER)/gp-hip-script-installer $(DESTDIR)$(PREFIX)/libexec/npclient/np-hip-script-installer
	$(SUDO) install -m 755 router/packaging/files/usr/libexec/npclient/vpnc-script $(DESTDIR)$(PREFIX)/libexec/npclient/vpnc-script
	$(SUDO) install -m 644 router/packaging/files/usr/share/polkit-1/actions/io.github.jahrulnr.nusaprotect.policy \
		$(DESTDIR)$(PREFIX)/share/polkit-1/actions/io.github.jahrulnr.nusaprotect.policy
	$(SUDO) install -m 755 router/packaging/files/usr/lib/NetworkManager/dispatcher.d/npclient-nm-hook \
		$(DESTDIR)$(PREFIX)/lib/NetworkManager/dispatcher.d/npclient-nm-hook
	$(SUDO) install -m 755 router/packaging/files/usr/lib/NetworkManager/dispatcher.d/pre-down.d/npclient.down \
		$(DESTDIR)$(PREFIX)/lib/NetworkManager/dispatcher.d/pre-down.d/npclient.down
	$(SUDO) install -m 644 packaging/nusaprotect.desktop $(DESTDIR)$(PREFIX)/share/applications/nusaprotect.desktop
	$(SUDO) install -m 644 src-tauri/icons/icon.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/nusaprotect.svg
	set -e; for size in 32x32 64x64 128x128; do \
		$(SUDO) install -d $(DESTDIR)$(PREFIX)/share/icons/hicolor/$$size/apps; \
		$(SUDO) install -m 644 src-tauri/icons/$$size.png $(DESTDIR)$(PREFIX)/share/icons/hicolor/$$size/apps/nusaprotect.png; \
	done
	$(SUDO) install -d $(DESTDIR)$(PREFIX)/share/icons/hicolor/256x256/apps $(DESTDIR)$(PREFIX)/share/icons/hicolor/512x512/apps
	$(SUDO) install -m 644 src-tauri/icons/128x128@2x.png $(DESTDIR)$(PREFIX)/share/icons/hicolor/256x256/apps/nusaprotect.png
	$(SUDO) install -m 644 src-tauri/icons/icon.png $(DESTDIR)$(PREFIX)/share/icons/hicolor/512x512/apps/nusaprotect.png
