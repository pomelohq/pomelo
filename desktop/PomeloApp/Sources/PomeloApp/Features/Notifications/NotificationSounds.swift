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

    // An event can hold several sounds; fire() plays a random one (no immediate repeat)
    // so a user who drops in a few clips gets variety.
    private var lastPlayed: [String: String] = [:]
    private func key(_ source: String, _ event: String) -> String { "notif.sounds.\(source).\(event)" }
    private func legacyKey(_ source: String, _ event: String) -> String { "notif.sound.\(source).\(event)" }
    private func defaultFor(_ event: String) -> [String] {
        switch event {
        case "finished", "onboarding_done": return ["sys:Glass"]
        case "needs_input": return ["sys:Ping"]
        case "service_crashed": return ["sys:Basso"]
        default: return []
        }
    }
    func sounds(_ source: String, _ event: String) -> [String] {
        if let data = d.data(forKey: key(source, event)), let arr = try? JSONDecoder().decode([String].self, from: data) {
            return arr
        }
        if let legacy = d.string(forKey: legacyKey(source, event)) { return legacy.isEmpty ? [] : [legacy] }
        return defaultFor(event)
    }
    func setSounds(_ values: [String], source: String, event: String) {
        d.set(try? JSONEncoder().encode(values), forKey: key(source, event)); objectWillChange.send()
    }
    func toggle(_ id: String, source: String, event: String) {
        var arr = sounds(source, event)
        if let i = arr.firstIndex(of: id) { arr.remove(at: i) } else { arr.append(id) }
        setSounds(arr, source: source, event: event)
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
    func fire(source: String, event: String) {
        let arr = sounds(source, event)
        guard !arr.isEmpty else { return }
        let k = "\(source).\(event)"
        var pick = arr.randomElement()!
        if arr.count > 1, pick == lastPlayed[k] { pick = arr.filter { $0 != lastPlayed[k] }.randomElement() ?? pick }
        lastPlayed[k] = pick
        play(pick)
    }
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
                            HStack(spacing: 10) {
                                EventSoundPicker(prefs: prefs, source: src.id, event: ev.id, options: optionIDs)
                                Button { prefs.fire(source: src.id, event: ev.id) } label: { Image(systemName: "play.circle").font(.system(size: 14)) }
                                    .buttonStyle(.plain).foregroundStyle(prefs.sounds(src.id, ev.id).isEmpty ? Theme.dim : Theme.accent)
                                    .disabled(prefs.sounds(src.id, ev.id).isEmpty)
                            }
                        }
                    }
                }
            }

            Section {
                Button { pickFile() } label: { Label("Add sound file...", systemImage: "plus") }
            } footer: {
                Text("Pick several per event and Pomelo plays a random one. Add your own .wav/.mp3/.aiff to use any voice.")
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .task { await loadAgents() }
        .onAppear { recheck() }
    }

    private var optionIDs: [String] { SoundPrefs.systemSounds.map { "sys:\($0)" } + prefs.customFiles.map { "file:\($0)" } }

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

func soundLabel(_ id: String) -> String {
    if id.hasPrefix("sys:") { return String(id.dropFirst(4)) }
    if id.hasPrefix("file:") { return String(id.dropFirst(5)) }
    return id
}

// Multi-select: an event can hold several sounds (Pomelo picks one at random).
private struct EventSoundPicker: View {
    @ObservedObject var prefs: SoundPrefs
    let source: String
    let event: String
    let options: [String]
    @State private var open = false

    private var selected: [String] { prefs.sounds(source, event) }
    private var summary: String {
        if selected.isEmpty { return "None" }
        return selected.count == 1 ? soundLabel(selected[0]) : "\(selected.count) sounds"
    }

    var body: some View {
        Button { open.toggle() } label: {
            HStack(spacing: 3) {
                Text(summary).font(Theme.mono(11)).lineLimit(1).truncationMode(.middle).frame(maxWidth: 150)
                Image(systemName: "chevron.down").font(.system(size: 6, weight: .bold)).opacity(0.7)
            }
            .foregroundStyle(selected.isEmpty ? Theme.fgMuted : Theme.accent)
            .padding(.horizontal, 8).padding(.vertical, 2.5)
            .background((selected.isEmpty ? Theme.fgMuted : Theme.accent).opacity(0.12), in: Capsule())
            .overlay(Capsule().strokeBorder((selected.isEmpty ? Theme.fgMuted : Theme.accent).opacity(0.35)))
        }
        .buttonStyle(.plain)
        .popover(isPresented: $open, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 1) {
                row(label: "None", checked: selected.isEmpty, previewable: false) {
                    prefs.setSounds([], source: source, event: event)
                }
                ForEach(options, id: \.self) { id in
                    row(label: soundLabel(id), checked: selected.contains(id), previewable: true, preview: { prefs.play(id) }) {
                        prefs.toggle(id, source: source, event: event)
                    }
                }
            }
            .padding(5).frame(minWidth: 200, maxWidth: 320).background(Theme.panel3)
        }
    }

    private func row(label: String, checked: Bool, previewable: Bool, preview: @escaping () -> Void = {}, toggle: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold))
                .foregroundStyle(Theme.accent).opacity(checked ? 1 : 0).frame(width: 12)
            Text(label).font(Theme.mono(11.5)).foregroundStyle(checked ? Theme.accent : Theme.fg)
                .lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 12)
            if previewable {
                Button(action: preview) { Image(systemName: "play.circle").font(.system(size: 12)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted)
            }
        }
        .padding(.horizontal, 8).padding(.vertical, 5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.hover.opacity(0.001))
        .contentShape(Rectangle())
        .onTapGesture(perform: toggle)
    }
}
