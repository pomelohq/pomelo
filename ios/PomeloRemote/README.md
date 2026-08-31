# Pomelo Remote (iOS)

A LAN companion app: monitor a Mac running Pomelo and nudge its agents from an
iPhone. It is a pure network client — no Go core, no libpom. It talks to the
Mac's remote control server (`internal/remote`) over TLS, trusting only the
pinned self-signed cert.

## What's here

`Sources/PomeloRemoteKit` is the whole app as a library target, verified by
building for the iOS Simulator (`xcodebuild build -scheme PomeloRemote
-destination 'platform=iOS Simulator,name=iPhone 17'`) and by `PairingTests`.

- `RemoteClient` — cert-pinned `URLSession` (SHA-256 leaf pin), `/rpc/{query,
  command,fetch}`, agent SSE stream, nudge.
- `PairedDevice` / `DeviceStore` — parse the QR payload, persist paired Macs.
- Views: `RootView` (paired Macs), `PairingView` (camera QR + paste),
  `DashboardView` (workspaces + agent state), `AgentView` (live output + nudge).

## Turning it into a runnable app (remaining, done in Xcode)

The `@main` entry lives in `App/PomeloRemoteApp.swift`, outside the SwiftPM
library. To ship:

1. New Xcode project > iOS App ("PomeloRemote"), SwiftUI lifecycle.
2. Add this SwiftPM package as a local dependency, link `PomeloRemoteKit`; use
   `App/PomeloRemoteApp.swift` as the app's entry (or copy its 8 lines).
3. Info.plist: add `NSCameraUsageDescription` ("Scan the pairing QR from
   Pomelo") — required for the QR scanner.
4. Signing: your Apple developer team; run on a device or the simulator.

## Pairing

On the Mac: Pomelo > Settings > Network > Remote control > enable. Scan the QR.
Everything the phone needs (host, port, token, cert fingerprint) is in it. To
use it off the LAN, put both devices on the same VPN.

## Not yet

- Bonjour discovery browse on the phone (the Mac already advertises
  `_pomelo._tcp`; the QR makes browse optional for pairing).
- Push notifications (agent finished / needs input) — needs APNs.
