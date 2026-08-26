import SwiftUI
import AppKit
import UniformTypeIdentifiers

struct NotifEvent: Identifiable, Hashable { let id: String; let title: String }
struct NotifSource: Identifiable, Hashable { let id: String; let title: String; let icon: String; let events: [NotifEvent] }

enum NotifCatalog {
    static let agentEvents = [
        NotifEvent(id: "finished", title: "Finished"),
        NotifEvent(id: "needs_input", title: "Needs your input"),
        NotifEvent(id: "compacting", title: "Compacting"),
    ]
    static let general = NotifSource(id: "general", title: "General", icon: "bell.fill", events: [
        NotifEvent(id: "service_crashed", title: "Service crashed"),
        NotifEvent(id: "onboarding_done", title: "Onboarding done"),
    ])
    static func sources(agents: [(id: String, name: String)]) -> [NotifSource] {
        agents.map { NotifSource(id: $0.id, title: $0.name, icon: "sparkles", events: agentEvents) } + [general]
    }
}

// Sound selection persists as a string: "" = none, "sys:<Name>" = a macOS system
// sound, "file:<name>" = a user-uploaded file in the sounds dir.
@MainActor
final class SoundPrefs: ObservableObject {
    static let shared = SoundPrefs()

    static let systemSounds = ["Basso", "Blow", "Bottle", "Frog", "Funk", "Glass", "Hero",
                               "Morse", "Ping", "Pop", "Purr", "Sosumi", "Submarine", "Tink"]

    @Published var whenFocused = UserDefaults.standard.bool(forKey: "notif.whenFocused") {
        didSet { UserDefaults.standard.set(whenFocused, forKey: "notif.whenFocused") }
    }
    @Published private(set) var customFiles: [String] = []

    private let d = UserDefaults.standard
    init() { customFiles = (try? FileManager.default.contentsOfDirectory(atPath: soundsDir.path))?.sorted() ?? [] }

    private func key(_ source: String, _ event: String) -> String { "notif.sound.\(source).\(event)" }
    private func defaultFor(_ event: String) -> String {
        switch event {
        case "finished", "onboarding_done": return "sys:Glass"
        case "needs_input": return "sys:Ping"
        case "service_crashed": return "sys:Basso"
        default: return ""
        }
    }
    func sound(_ source: String, _ event: String) -> String { d.string(forKey: key(source, event)) ?? defaultFor(event) }
    func setSound(_ value: String, source: String, event: String) {
        d.set(value, forKey: key(source, event)); objectWillChange.send()
    }

    var soundsDir: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Pomelo/sounds", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return base
    }
    func importFile(_ url: URL) {
        let dest = soundsDir.appendingPathComponent(url.lastPathComponent)
        try? FileManager.default.removeItem(at: dest)
        try? FileManager.default.copyItem(at: url, to: dest)
        customFiles = (try? FileManager.default.contentsOfDirectory(atPath: soundsDir.path))?.sorted() ?? []
    }

    func play(_ storage: String) {
        if storage.hasPrefix("sys:") {
            NSSound(named: String(storage.dropFirst(4)))?.play()
        } else if storage.hasPrefix("file:") {
            NSSound(contentsOf: soundsDir.appendingPathComponent(String(storage.dropFirst(5))), byReference: true)?.play()
        }
    }
    func fire(source: String, event: String) { play(sound(source, event)) }
}

struct NotificationsSettings: View {
    @EnvironmentObject var state: AppState
    @StateObject private var prefs = SoundPrefs.shared
    @State private var sources: [NotifSource] = []
    @State private var notifOK = true

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                delivery
                ForEach(sources) { src in sourceSection(src) }
                addSoundRow
            }
            .padding(.vertical, 4)
        }
        .task { await loadAgents() }
        .onAppear { Notifier.currentlyAuthorized { notifOK = $0 } }
    }

    private var delivery: some View {
        VStack(alignment: .leading, spacing: 8) {
            Toggle("Notify on Claude activity", isOn: $state.notifyClaude)
                .onChange(of: state.notifyClaude) { if state.notifyClaude { Notifier.promptOrOpenSettings() } }
            Toggle("Alert even while I'm viewing that workspace", isOn: $prefs.whenFocused)
            Text("Turn this on when you leave a run going and want to hear it finish.")
                .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            if state.notifyClaude && !notifOK {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 11)).foregroundStyle(Theme.warn)
                    Text("macOS hasn't granted notification permission").font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
                    Button("Grant") { Notifier.promptOrOpenSettings() }.buttonStyle(.plain).foregroundStyle(Theme.accent)
                }
            }
            Button { Notifier.sendTest() } label: { Text("Send test notification").font(.system(size: 12)) }
                .buttonStyle(.plain).foregroundStyle(Theme.accent)
        }
    }

    private func sourceSection(_ src: NotifSource) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 7) {
                Image(systemName: src.icon).font(.system(size: 11)).foregroundStyle(Theme.accent)
                Text(src.title.uppercased()).font(.system(size: 10.5, weight: .semibold)).kerning(0.5).foregroundStyle(Theme.muted)
            }
            ForEach(src.events) { ev in eventRow(src.id, ev) }
        }
    }

    private func eventRow(_ source: String, _ ev: NotifEvent) -> some View {
        let current = prefs.sound(source, ev.id)
        return HStack(spacing: 10) {
            Text(ev.title).font(.system(size: 12.5)).foregroundStyle(Theme.fg).frame(width: 150, alignment: .leading)
            Picker("", selection: Binding(get: { current }, set: { prefs.setSound($0, source: source, event: ev.id) })) {
                Text("None").tag("")
                ForEach(SoundPrefs.systemSounds, id: \.self) { Text($0).tag("sys:\($0)") }
                if !prefs.customFiles.isEmpty {
                    Divider()
                    ForEach(prefs.customFiles, id: \.self) { Text($0).tag("file:\($0)") }
                }
            }
            .labelsHidden().frame(width: 180)
            Button { prefs.play(current) } label: { Image(systemName: "play.circle").font(.system(size: 14)) }
                .buttonStyle(.plain).foregroundStyle(current.isEmpty ? Theme.dim : Theme.accent)
                .disabled(current.isEmpty)
            Spacer()
        }
    }

    private var addSoundRow: some View {
        Button { pickFile() } label: {
            Label("Add sound file...", systemImage: "plus").font(.system(size: 12))
        }.buttonStyle(.plain).foregroundStyle(Theme.accent)
    }

    private func pickFile() {
        let p = NSOpenPanel()
        p.allowedContentTypes = [.audio]
        p.allowsMultipleSelection = false
        if p.runModal() == .OK, let url = p.url { prefs.importFile(url) }
    }

    private func loadAgents() async {
        struct A: Decodable { var id = ""; var name = "" }
        let d = await Task.detached { PomCore.shared.codeAgentsData() }.value
        let agents = (PomJSON.decode([A].self, from: d) ?? []).map { (id: $0.id, name: $0.name) }
        sources = NotifCatalog.sources(agents: agents.isEmpty ? [("claude", "Claude Code")] : agents)
    }
}
