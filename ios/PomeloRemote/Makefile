-include Makefile.local

XCODE            ?= /Applications/Xcode-beta.app
export DEVELOPER_DIR := $(XCODE)/Contents/Developer

PROJECT          := PomeloRemote.xcodeproj
SCHEME           := PomeloRemote
APPLE_ID         ?=
DEVICE           ?=
KEYCHAIN_ITEM    ?= PomeloRemote-ASC
SIM              ?= iPhone 17
BUILD            ?= $(shell date +%y%m%d%H%M)

BUILDDIR         := build
ARCHIVE          := $(BUILDDIR)/PomeloRemote.xcarchive
EXPORTDIR        := $(BUILDDIR)/export
IPA              := $(EXPORTDIR)/PomeloRemote.ipa
XCFLAGS          := -skipPackagePluginValidation -allowProvisioningUpdates

.PHONY: sim device archive ipa upload testflight clean

device:
	xcodebuild -project $(PROJECT) -scheme $(SCHEME) \
	  -destination 'id=$(DEVICE)' -configuration Debug \
	  -derivedDataPath build/dd $(XCFLAGS) build
	xcrun devicectl device install app --device $(DEVICE) \
	  build/dd/Build/Products/Debug-iphoneos/PomeloRemote.app

sim:
	xcodebuild -project $(PROJECT) -scheme $(SCHEME) \
	  -destination 'platform=iOS Simulator,name=$(SIM)' \
	  -skipPackagePluginValidation build
	xcrun simctl install booted \
	  "$$(find ~/Library/Developer/Xcode/DerivedData/PomeloRemote-*/Build/Products/Debug-iphonesimulator -maxdepth 1 -name PomeloRemote.app | head -1)"
	xcrun simctl launch booted app.pomelo.remote

archive:
	rm -rf $(ARCHIVE)
	xcodebuild -project $(PROJECT) -scheme $(SCHEME) \
	  -destination 'generic/platform=iOS' \
	  -archivePath $(ARCHIVE) \
	  CURRENT_PROJECT_VERSION=$(BUILD) \
	  $(XCFLAGS) archive
	@echo "archived build $(BUILD)"

ipa: archive
	rm -rf $(EXPORTDIR)
	xcodebuild -exportArchive \
	  -archivePath $(ARCHIVE) \
	  -exportOptionsPlist ExportOptions.plist \
	  -exportPath $(EXPORTDIR) \
	  -allowProvisioningUpdates
	@echo "exported $(IPA)"

upload:
	xcrun altool --upload-app -f $(IPA) -t ios \
	  -u $(APPLE_ID) -p @keychain:$(KEYCHAIN_ITEM)

testflight: ipa upload
	@echo "uploaded build $(BUILD) to TestFlight"

clean:
	rm -rf $(BUILDDIR)
