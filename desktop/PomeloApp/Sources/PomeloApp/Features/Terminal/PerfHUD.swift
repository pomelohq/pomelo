import SwiftUI
import QuartzCore
import Foundation

// In-app performance HUD + file logger. Two signals that together locate UI lag:
//  - per-view body evaluations per second (a view re-rendering far more than it
//    should is the classic over-observation / god-object symptom) and named
//    "set:<prop>" counters for AppState mutations (which poll drives the fan-out).
//  - main-thread stall: a 120 Hz timer measures its own scheduling jitter, so a
//    blocked main thread shows up as a spike in the gap between fires.
// When logging is on, each 1 s sample is appended to /tmp/pom-perf.log so a run can
// be investigated after the fact without watching the on-screen overlay.
final class PerfHUD: ObservableObject {
    static let shared = PerfHUD()

    @Published var rows: [(String, Int)] = []
    @Published var stallMs: Double = 0
    @Published var visible = false

    private var collecting = false
    private var counts: [String: Int] = [:]
    private var timer: Timer?
    private var lastTick = CACurrentMediaTime()
    private var lastSecond = CACurrentMediaTime()
    private var worst: Double = 0
    private let logPath = "/tmp/pom-perf.log"
    private var logHandle: FileHandle?

    func tick(_ name: String) { if collecting || visible { counts[name, default: 0] += 1 } }

    // Opt-in file logging: only while the HUD is on, so a normal run never writes to
    // the user's disk. Off deletes the file so nothing lingers.
    func startLogging() {
        FileManager.default.createFile(atPath: logPath, contents: nil)
        logHandle = FileHandle(forWritingAtPath: logPath)
        collecting = true
        ensureTimer()
    }

    func stopLogging() {
        try? logHandle?.close(); logHandle = nil
        collecting = false
        try? FileManager.default.removeItem(atPath: logPath)
    }


    private func ensureTimer() {
        guard timer == nil else { return }
        lastTick = CACurrentMediaTime(); lastSecond = lastTick
        let t = Timer(timeInterval: 1.0 / 120.0, repeats: true) { [weak self] _ in self?.sample() }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    private func sample() {
        let now = CACurrentMediaTime()
        worst = max(worst, (now - lastTick) * 1000)
        lastTick = now
        guard now - lastSecond >= 1 else { return }
        lastSecond = now
        let sorted = counts.map { ($0.key, $0.value) }.sorted { $0.1 > $1.1 }
        rows = sorted
        stallMs = worst
        writeLog(stall: worst, rows: sorted)
        worst = 0
        counts.removeAll(keepingCapacity: true)
    }

    private func writeLog(stall: Double, rows: [(String, Int)]) {
        guard let h = logHandle else { return }
        let top = rows.prefix(24).map { "\($0.0)=\($0.1)" }.joined(separator: " ")
        let ts = Self.logFmt.string(from: Date())
        let line = String(format: "%@ stall=%.1fms %@\n", ts, stall, top)
        if let d = line.data(using: .utf8) { h.write(d) }
    }
    private static let logFmt: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "HH:mm:ss"; return f
    }()
}

extension View {
    // Runs each time the enclosing view's body is evaluated.
    func perfTag(_ name: String) -> some View {
        PerfHUD.shared.tick(name)
        return self
    }
}

struct PerfHUDOverlay: View {
    @ObservedObject private var hud = PerfHUD.shared
    var body: some View {
        if hud.visible {
            VStack(alignment: .leading, spacing: 1) {
                Text(String(format: "main stall %.1f ms", hud.stallMs))
                    .foregroundStyle(hud.stallMs > 30 ? .red : (hud.stallMs > 12 ? .yellow : .green))
                ForEach(hud.rows.prefix(18), id: \.0) { r in
                    Text("\(r.1)/s  \(r.0)").foregroundStyle(r.1 > 40 ? .orange : .white)
                }
            }
            .font(.system(size: 10, weight: .medium, design: .monospaced))
            .padding(6)
            .background(.black.opacity(0.62), in: RoundedRectangle(cornerRadius: 6))
            .allowsHitTesting(false)
            .padding(8)
        }
    }
}

struct PerfHUDToggle: View {
    @ObservedObject private var hud = PerfHUD.shared
    var body: some View {
        Button(hud.visible ? "Hide Perf HUD" : "Show Perf HUD") {
            hud.visible.toggle()
            if hud.visible { hud.startLogging() } else { hud.stopLogging() }
        }
        .keyboardShortcut("p", modifiers: [.command, .option, .control])
    }
}
