import SwiftUI
import AppKit

@MainActor
final class SecretsViewModel: ObservableObject {
    @Published private(set) var names: [String] = []
    @Published var newName = ""
    @Published var newValue = ""
    private let api: SecretsAPI
    init(api: SecretsAPI = PomCore.shared) { self.api = api }

    func load() async {
        let d = await api.call { $0.secretNamesData() }
        struct R: Decodable { var names: [String]? }
        names = (PomJSON.decode(R.self, from: d)?.names ?? []).sorted()
    }

    func add() async {
        let n = newName.trimmingCharacters(in: .whitespaces)
        let v = newValue
        guard !n.isEmpty, !v.isEmpty else { return }
        _ = await api.call { $0.secretSet(name: n, value: v) }
        newName = ""; newValue = ""
        await load()
    }

    // Storing an empty value deletes the secret (matches the core convention).
    func remove(_ name: String) async {
        _ = await api.call { $0.secretSet(name: name, value: "") }
        await load()
    }

    func copyValue(_ name: String) async {
        let d = await api.call { $0.secretGet(name: name) }
        struct R: Decodable { var value = "" }
        let v = PomJSON.decode(R.self, from: d)?.value ?? ""
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(v, forType: .string)
    }
}

struct SecretsView: View {
    @StateObject private var vm = SecretsViewModel()

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Encrypted app-local values for {{secret.NAME}} referenced in your config. Names are listed; values are never shown here.")
                .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, 8)
            Divider().overlay(Theme.borderSoft)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    if vm.names.isEmpty {
                        Text("No secrets yet.").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                            .padding(.horizontal, 16).padding(.vertical, 10)
                    }
                    ForEach(vm.names, id: \.self) { name in
                        HStack(spacing: 8) {
                            Text("{{secret.\(name)}}").font(Theme.mono(12)).foregroundStyle(Theme.tool).lineLimit(1)
                            Spacer(minLength: 0)
                            Button { Task { await vm.copyValue(name) } } label: {
                                Image(systemName: "doc.on.doc").font(.system(size: 10))
                            }.buttonStyle(.plain).foregroundStyle(Theme.fgMuted).help("Copy value to clipboard")
                            Button { Task { await vm.remove(name) } } label: {
                                Image(systemName: "trash").font(.system(size: 10))
                            }.buttonStyle(.plain).foregroundStyle(Theme.danger).help("Delete secret")
                        }
                        .padding(.horizontal, 16).padding(.vertical, 6)
                    }
                }
                .padding(.vertical, 4)
            }
            Divider().overlay(Theme.borderSoft)
            HStack(spacing: 8) {
                TextField("NAME", text: $vm.newName).textFieldStyle(.roundedBorder).frame(width: 200)
                SecureField("value", text: $vm.newValue).textFieldStyle(.roundedBorder)
                Button("Add") { Task { await vm.add() } }
                    .buttonStyle(.borderedProminent).tint(Theme.accent)
                    .disabled(vm.newName.trimmingCharacters(in: .whitespaces).isEmpty || vm.newValue.isEmpty)
            }
            .padding(.horizontal, 16).padding(.vertical, 12)
        }
        .task { await vm.load() }
    }
}
