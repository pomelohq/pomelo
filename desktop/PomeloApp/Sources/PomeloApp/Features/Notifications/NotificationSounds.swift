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

    // A "set" is a named collection of event->sounds mappings; switching sets swaps all
    // the picks at once, so a user can keep e.g. a quiet set and a loud one.
    struct SoundSet: Identifiable, Codable, Hashable { var id: String; var name: String }
    @Published private(set) var sets: [SoundSet] = []
    @Published var activeSet: String = "default" {
        didSet { UserDefaults.standard.set(activeSet, forKey: "notif.activeSet"); objectWillChange.send() }
    }

    private let d = UserDefaults.standard
    init() {
        refreshFiles()
        if let data = d.data(forKey: "notif.sets"), let s = try? JSONDecoder().decode([SoundSet].self, from: data), !s.isEmpty {
            sets = s
        } else {
            sets = [SoundSet(id: "default", name: "Default")]
        }
        activeSet = d.string(forKey: "notif.activeSet").flatMap { id in sets.contains { $0.id == id } ? id : nil } ?? sets[0].id
    }

    var activeSetName: String { sets.first { $0.id == activeSet }?.name ?? "Default" }

    private func persistSets() { d.set(try? JSONEncoder().encode(sets), forKey: "notif.sets") }
    func addSet(_ name: String) {
        let id = UUID().uuidString
        let prefix = "notif.set.\(activeSet).sounds."
        for (k, v) in d.dictionaryRepresentation() where k.hasPrefix(prefix) {
            d.set(v, forKey: "notif.set.\(id).sounds.\(k.dropFirst(prefix.count))")
        }
        sets.append(SoundSet(id: id, name: name)); persistSets(); activeSet = id
    }
    func renameActiveSet(_ name: String) {
        if let i = sets.firstIndex(where: { $0.id == activeSet }) { sets[i].name = name; persistSets() }
    }
    func deleteActiveSet() {
        guard sets.count > 1 else { return }
        let gone = activeSet
        sets.removeAll { $0.id == gone }; persistSets()
        activeSet = sets[0].id
    }

    private func valid(_ id: String) -> Bool {
        id.hasPrefix("sys:") || (id.hasPrefix("file:") && customFiles.contains(String(id.dropFirst(5))))
    }

    // An event can hold several sounds; fire() plays a random one (no immediate repeat)
    // so a user who drops in a few clips gets variety.
    private var lastPlayed: [String: String] = [:]
    private func key(_ source: String, _ event: String) -> String { "notif.set.\(activeSet).sounds.\(source).\(event)" }
    private func preSetKey(_ source: String, _ event: String) -> String { "notif.sounds.\(source).\(event)" }
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
            return arr.filter(valid)
        }
        // Migrate the pre-set (single, global) mapping into the default set on first read.
        if activeSet == "default", let data = d.data(forKey: preSetKey(source, event)), let arr = try? JSONDecoder().decode([String].self, from: data) {
            return arr.filter(valid)
        }
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
        refreshFiles()
    }
    func deleteFile(_ name: String) {
        try? FileManager.default.removeItem(at: soundsDir.appendingPathComponent(name))
        refreshFiles()
    }
    private func refreshFiles() {
        customFiles = ((try? FileManager.default.contentsOfDirectory(atPath: soundsDir.path)) ?? [])
            .filter { !$0.hasPrefix(".") }.sorted()
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
    @State private var showNewSet = false
    @State private var showRename = false
    @State private var setName = ""

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
                    .disabled(!state.notifyClaude)
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
                Text("The master switch. Alert-when-viewing also chimes while you're on that workspace. Needs macOS notification permission.")
            }

            Section {
                LabeledContent("Active set") {
                    HStack(spacing: 8) {
                        ChipSelect(text: clip(prefs.activeSetName), color: Theme.accent, options: prefs.sets.map(\.name),
                                   current: prefs.activeSetName) { name in
                            if let s = prefs.sets.first(where: { $0.name == name }) { prefs.activeSet = s.id }
                        }
                        Button { setName = ""; showNewSet = true } label: { Image(systemName: "plus.circle") }
                            .buttonStyle(.plain).foregroundStyle(Theme.accent).help("New set")
                        Button { setName = prefs.activeSetName; showRename = true } label: { Image(systemName: "pencil") }
                            .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).help("Rename set")
                        Button { prefs.deleteActiveSet() } label: { Image(systemName: "trash") }
                            .buttonStyle(.plain).foregroundStyle(Theme.dim).disabled(prefs.sets.count <= 1).help("Delete set")
                    }
                }
            } header: { Text("Sound set") } footer: {
                Text("Save different sound line-ups as sets and switch between them.")
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
        .alert("New sound set", isPresented: $showNewSet) {
            TextField("Name", text: $setName)
            Button("Create") { let n = setName.trimmingCharacters(in: .whitespaces); if !n.isEmpty { prefs.addSet(n) } }
            Button("Cancel", role: .cancel) {}
        } message: { Text("Starts as a copy of the current picks; edit them below.") }
        .alert("Rename set", isPresented: $showRename) {
            TextField("Name", text: $setName)
            Button("Save") { let n = setName.trimmingCharacters(in: .whitespaces); if !n.isEmpty { prefs.renameActiveSet(n) } }
            Button("Cancel", role: .cancel) {}
        }
    }

    private var optionIDs: [String] { prefs.customFiles.map { "file:\($0)" } + SoundPrefs.systemSounds.map { "sys:\($0)" } }

    private func pickFile() {
        let p = NSOpenPanel()
        p.allowedContentTypes = [.audio]
        p.allowsMultipleSelection = true
        if p.runModal() == .OK { for url in p.urls { prefs.importFile(url) } }
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

// Middle-clip a long name so a pill stays compact without a greedy fixed-width frame.
func clip(_ s: String, _ n: Int = 22) -> String {
    guard s.count > n else { return s }
    return String(s.prefix(n / 2)) + "..." + String(s.suffix(n / 2 - 2))
}

// Multi-select: an event can hold several sounds (Pomelo picks one at random).
private struct EventSoundPicker: View {
    @ObservedObject var prefs: SoundPrefs
    let source: String
    let event: String
    let options: [String]
    @State private var open = false
    @State private var search = ""

    private var selected: [String] { prefs.sounds(source, event) }
    private var filtered: [String] {
        search.isEmpty ? options : options.filter { soundLabel($0).localizedCaseInsensitiveContains(search) }
    }
    private var summary: String {
        if selected.isEmpty { return "None" }
        return selected.count == 1 ? soundLabel(selected[0]) : "\(selected.count) sounds"
    }

    var body: some View {
        Button { open.toggle() } label: {
            HStack(spacing: 3) {
                Text(clip(summary)).font(Theme.mono(11)).lineLimit(1).truncationMode(.middle)
                    .frame(width: 92, alignment: .leading)
                Image(systemName: "chevron.down").font(.system(size: 6, weight: .bold)).opacity(0.7)
            }
            .foregroundStyle(selected.isEmpty ? Theme.fgMuted : Theme.accent)
            .padding(.horizontal, 8).padding(.vertical, 2.5)
            .background((selected.isEmpty ? Theme.fgMuted : Theme.accent).opacity(0.12), in: Capsule())
            .overlay(Capsule().strokeBorder((selected.isEmpty ? Theme.fgMuted : Theme.accent).opacity(0.35)))
        }
        .buttonStyle(.plain)
        .popover(isPresented: $open, arrowEdge: .bottom) {
            VStack(spacing: 0) {
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").font(.system(size: 10)).foregroundStyle(Theme.dim)
                    TextField("Filter sounds", text: $search).textFieldStyle(.plain).font(.system(size: 12))
                }
                .padding(.horizontal, 10).padding(.vertical, 7)
                Divider().overlay(Theme.borderSoft)
                ScrollView {
                    VStack(alignment: .leading, spacing: 1) {
                        if search.isEmpty {
                            row(label: "None", checked: selected.isEmpty, previewable: false) {
                                prefs.setSounds([], source: source, event: event)
                            }
                        }
                        ForEach(filtered, id: \.self) { id in
                            row(label: soundLabel(id), checked: selected.contains(id), previewable: true,
                                preview: { prefs.play(id) },
                                onDelete: id.hasPrefix("file:") ? { prefs.deleteFile(String(id.dropFirst(5))) } : nil) {
                                prefs.toggle(id, source: source, event: event)
                            }
                        }
                    }
                    .padding(5)
                }
            }
            .frame(minWidth: 240, maxWidth: 340, maxHeight: 340).background(Theme.panel3)
            .onDisappear { search = "" }
        }
    }

    private func row(label: String, checked: Bool, previewable: Bool, preview: @escaping () -> Void = {},
                     onDelete: (() -> Void)? = nil, toggle: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold))
                .foregroundStyle(Theme.accent).opacity(checked ? 1 : 0).frame(width: 12)
            Text(label).font(Theme.mono(11.5)).foregroundStyle(checked ? Theme.accent : Theme.fg)
                .lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 12)
            if let onDelete {
                Button(action: onDelete) { Image(systemName: "trash").font(.system(size: 10)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.dim)
            }
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
