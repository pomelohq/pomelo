import SwiftUI
import AppKit
import UniformTypeIdentifiers

struct NotifEvent: Identifiable, Hashable { let id: String; let title: String }
struct NotifSource: Identifiable, Hashable { let id: String; let title: String; let icon: String; let events: [NotifEvent] }

enum NotifCatalog {
    static let agentEvents = [
        NotifEvent(id: "working", title: "Started working"),
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

    private func recheck() {
        Notifier.currentlyAuthorized { notifOK = $0 }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { Notifier.currentlyAuthorized { notifOK = $0 } }
    }

    var body: some View {
        Form {
            Section {
                Toggle("Notify on Claude activity", isOn: $state.notifyClaude)
                    .onChange(of: state.notifyClaude) { if state.notifyClaude { Notifier.promptOrOpenSettings(); recheck() } }
                Toggle("Alert even while I'm viewing that workspace", isOn: $prefs.whenFocused)
                if state.notifyClaude && !notifOK {
                    HStack(spacing: 8) {
                        Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 11)).foregroundStyle(Theme.warn)
                        Text("macOS hasn't granted notification permission").font(.system(size: 11.5)).foregroundStyle(Theme.fgMuted)
                        Spacer()
                        Button("Grant") { Notifier.promptOrOpenSettings(); recheck() }.controlSize(.small)
                    }
                }
                Button("Send test notification") { Notifier.sendTest(); recheck() }.controlSize(.small)
            } header: { Text("Delivery") } footer: {
                Text("Alert-when-viewing is handy when you leave a run going and want to hear it finish. Needs macOS notification permission.")
            }

            ForEach(sources) { src in
                Section(src.title) {
                    ForEach(src.events) { ev in
                        LabeledContent(ev.title) {
                            HStack(spacing: 8) {
                                Picker("", selection: binding(src.id, ev.id)) {
                                    Text("None").tag("")
                                    ForEach(SoundPrefs.systemSounds, id: \.self) { Text($0).tag("sys:\($0)") }
                                    ForEach(prefs.customFiles, id: \.self) { Text($0).tag("file:\($0)") }
                                }
                                .labelsHidden().frame(width: 150)
                                Button { prefs.play(prefs.sound(src.id, ev.id)) } label: { Image(systemName: "play.circle") }
                                    .buttonStyle(.plain).foregroundStyle(prefs.sound(src.id, ev.id).isEmpty ? Theme.dim : Theme.accent)
                                    .disabled(prefs.sound(src.id, ev.id).isEmpty)
                            }
                        }
                    }
                }
            }

            Section {
                Button { pickFile() } label: { Label("Add sound file...", systemImage: "plus") }
            } footer: {
                Text("Use your own .wav/.mp3/.aiff — uploaded sounds appear in every dropdown.")
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .task { await loadAgents() }
        .onAppear { recheck() }
    }

    private func binding(_ source: String, _ event: String) -> Binding<String> {
        Binding(get: { prefs.sound(source, event) }, set: { prefs.setSound($0, source: source, event: event) })
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
