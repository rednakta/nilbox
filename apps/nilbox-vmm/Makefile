ENTITLEMENTS = nilbox-vmm.entitlements
BINARY = .build/release/nilbox-vmm
TAURI_BINARIES = ../apps/nilbox/src-tauri/binaries

release:
	swift build -c release
	codesign --sign - --entitlements $(ENTITLEMENTS) --force $(BINARY)
	@echo "✓ nilbox-vmm built and signed"

release-arm64:
	swift build -c release --arch arm64
	codesign --sign - --entitlements $(ENTITLEMENTS) --force .build/arm64-apple-macosx/release/nilbox-vmm
	@echo "✓ nilbox-vmm (arm64) built and signed"

release-x86_64:
	swift build -c release --arch x86_64
	codesign --sign - --entitlements $(ENTITLEMENTS) --force .build/x86_64-apple-macosx/release/nilbox-vmm
	@echo "✓ nilbox-vmm (x86_64) built and signed"

release-universal: release-arm64 release-x86_64
	mkdir -p .build/release
	lipo -create \
		.build/arm64-apple-macosx/release/nilbox-vmm \
		.build/x86_64-apple-macosx/release/nilbox-vmm \
		-output .build/release/nilbox-vmm
	codesign --sign - --entitlements $(ENTITLEMENTS) --force .build/release/nilbox-vmm
	@echo "✓ nilbox-vmm (universal) built and signed"

build:
	swift build
	codesign --sign - --entitlements $(ENTITLEMENTS) --force .build/debug/nilbox-vmm
	@echo "✓ nilbox-vmm (debug) built and signed"

install: release
	mkdir -p $(TAURI_BINARIES)
	cp $(BINARY) $(TAURI_BINARIES)/nilbox-vmm-aarch64-apple-darwin
	@echo "✓ copied to $(TAURI_BINARIES)/nilbox-vmm-aarch64-apple-darwin"

install-arm64: release-arm64
	mkdir -p $(TAURI_BINARIES)
	cp .build/arm64-apple-macosx/release/nilbox-vmm $(TAURI_BINARIES)/nilbox-vmm-aarch64-apple-darwin
	@echo "✓ copied to $(TAURI_BINARIES)/nilbox-vmm-aarch64-apple-darwin"

install-x86_64: release-x86_64
	mkdir -p $(TAURI_BINARIES)
	cp .build/x86_64-apple-macosx/release/nilbox-vmm $(TAURI_BINARIES)/nilbox-vmm-x86_64-apple-darwin
	@echo "✓ copied to $(TAURI_BINARIES)/nilbox-vmm-x86_64-apple-darwin"

install-universal: release-universal
	mkdir -p $(TAURI_BINARIES)
	cp .build/release/nilbox-vmm $(TAURI_BINARIES)/nilbox-vmm-universal-apple-darwin
	@echo "✓ copied to $(TAURI_BINARIES)/nilbox-vmm-universal-apple-darwin"

.PHONY: release release-arm64 release-x86_64 release-universal build install install-arm64 install-x86_64 install-universal
