# Local, untracked overrides for release identity (signing cert, notary profile,
# gh accounts, release repo). Copy Makefile.local.example → Makefile.local.
-include Makefile.local

VERSION = $(shell grep '^const version' cmd/pom/root.go | cut -d'"' -f2)
MAJOR = $(word 1,$(subst ., ,$(VERSION)))
MINOR = $(word 2,$(subst ., ,$(VERSION)))
PATCH = $(word 3,$(subst ., ,$(VERSION)))

BINARY = pom
LDFLAGS = -s -w
# Signing identity. Default is adhoc ("-") so anyone can build locally; set a real
# "Developer ID Application: …" in Makefile.local for notarizable release builds.
SIGN_ID ?= -
export SIGN_ID NOTARY_PROFILE GH_USER_PUBLISH GH_USER_BACK RELEASE_REPO

APP_VERSION = $(shell grep '^const appVersion' cmd/libpom/libpom.go | cut -d'"' -f2)

.PHONY: build dev release install clean test vet check check-strict patch minor major app-publish version-check dmg dev-app

# --- Development ---

build:
	go build -o $(BINARY) ./cmd/pom/

dev:
	go build -o $(BINARY) ./cmd/pom/ && ./$(BINARY) $(ARGS)

test:
	go test ./...

vet:
	go vet ./...

# CLAUDE.md coding-rule checker: security hard-fails, comment-bloat report.
# `make check` reports; `make check-strict` also fails on comment bloat.
check:
	python3 scripts/check_rules.py

check-strict:
	python3 scripts/check_rules.py --strict

# --- Local release (install to /usr/local/bin) ---

release:
	go build -ldflags "$(LDFLAGS)" -o $(BINARY) ./cmd/pom/
	codesign -s "$(SIGN_ID)" --identifier com.pomelo.pom --force --options runtime $(BINARY)

install: release
	sudo cp $(BINARY) /usr/local/bin/$(BINARY)
	@echo "Installed pom v$(VERSION) to /usr/local/bin/$(BINARY)"

# --- Version bump + release to GitHub ---
# ONE version for both the CLI (pom) and the native app; keep the two consts in
# lockstep (appVersion drives the DMG name + Sparkle appcast). Convention:
#   patch = bug fix / no new surface   minor = new feature   major = breaking
# make patch  ->  0.10.8 -> 0.10.9 -> tag -> push   (then: make app-publish)
# make minor  ->  0.10.8 -> 0.11.0 -> tag -> push
# make major  ->  0.10.8 ->  1.0.0 -> tag -> push

# Fail loudly if the two version constants ever drift (a bump touched only one).
version-check:
	@[ "$(VERSION)" = "$(APP_VERSION)" ] || { echo "version drift: pom=$(VERSION) libpom=$(APP_VERSION)"; exit 1; }

patch:
	$(eval NEW_VERSION := $(MAJOR).$(MINOR).$(shell echo $$(($(PATCH)+1))))
	@$(MAKE) _release NEW_VERSION=$(NEW_VERSION)

minor:
	$(eval NEW_VERSION := $(MAJOR).$(shell echo $$(($(MINOR)+1))).0)
	@$(MAKE) _release NEW_VERSION=$(NEW_VERSION)

major:
	$(eval NEW_VERSION := $(shell echo $$(($(MAJOR)+1))).0.0)
	@$(MAKE) _release NEW_VERSION=$(NEW_VERSION)

_release: version-check
	@echo "$(VERSION) -> $(NEW_VERSION)"
	sed -i '' 's/const version = "$(VERSION)"/const version = "$(NEW_VERSION)"/' cmd/pom/root.go
	sed -i '' 's/const appVersion = "$(VERSION)"/const appVersion = "$(NEW_VERSION)"/' cmd/libpom/libpom.go
	@grep -q 'appVersion = "$(NEW_VERSION)"' cmd/libpom/libpom.go || { echo "libpom appVersion not bumped"; exit 1; }
	git add cmd/pom/root.go cmd/libpom/libpom.go
	git commit -m "release: v$(NEW_VERSION)"
	git tag v$(NEW_VERSION)
	git push origin main v$(NEW_VERSION)
	@echo ""
	@echo "v$(NEW_VERSION) tagged & pushed. GitHub Actions (release.yml) now builds"
	@echo "and publishes the notarized DMG + appcast to the release. Watch:"
	@echo "  gh run watch --repo pomelohq/pomelo"
	@echo "(Local fallback without CI: make app-publish)"

# --- Native macOS app (.app → signed + notarized DMG) ---
# make dmg            -> build + sign + notarize + staple -> desktop/PomeloApp/dist/Pomelo-<v>.dmg
# make dmg DRY_RUN=1  -> build + sign + assemble DMG, skip notarization
dmg:
	@DRY_RUN=$(DRY_RUN) bash desktop/PomeloApp/package.sh

# Local dev .app you can actually click through — Debug build, no hardened
# runtime, adhoc-signed, opens automatically. `make dmg`'s hardened-runtime
# .app won't launch with an adhoc identity (dyld refuses to load
# Sparkle.framework across two independently-adhoc-signed binaries); this
# skips that entirely. Never ship this build.
dev-app:
	@bash desktop/PomeloApp/dev-app.sh

# Native app release: build+notarize DMG, then create the pomelo-releases GitHub
# release with the DMG + Sparkle appcast + checksums (keeps old releases, switches
# gh accounts). Run after `make patch`. `make app-publish DRY_RUN=1` builds only.
app-publish: version-check
	@DRY_RUN=$(DRY_RUN) RELEASE_NOTES_FILE="$(RELEASE_NOTES_FILE)" RELEASE_NOTES="$(RELEASE_NOTES)" bash scripts/release-app.sh

clean:
	rm -f $(BINARY)
	rm -rf dist/
	rm -rf desktop/PomeloApp/dist-dev/
