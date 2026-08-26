import SwiftUI
import AppKit

@MainActor
final class SecretsViewModel: ObservableObject {
    @Published private(set) var names: [String] = []
    @Published private(set) var revealed: [String: String] = [:]
    @Published var newName = ""
    @Published var newValue = ""
    private let api: SecretsAPI
    init(api: SecretsAPI = PomCore.shared) { self.api = api }

    func toggleReveal(_ name: String) async {
        if revealed[name] != nil { revealed[name] = nil; return }
        let d = await api.call { $0.secretGet(name: name) }
        struct R: Decodable { var value = "" }
        revealed[name] = PomJSON.decode(R.self, from: d)?.value ?? ""
    }

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
                LazyVStack(alignment: .leading, spacing: 0) {
                    if vm.names.isEmpty {
                        Text("No secrets yet.").font(.system(size: 12)).foregroundStyle(Theme.fgMuted)
                            .padding(.horizontal, 16).padding(.vertical, 12)
                    }
                    ForEach(Array(vm.names.enumerated()), id: \.element) { idx, name in
                        SecretRow(name: name, value: vm.revealed[name],
                                  onReveal: { Task { await vm.toggleReveal(name) } },
                                  onCopy: { Task { await vm.copyValue(name) } },
                                  onDelete: { Task { await vm.remove(name) } })
                        if idx < vm.names.count - 1 {
                            Divider().overlay(Theme.borderSoft).padding(.leading, 16)
                        }
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

private struct SecretRow: View {
    let name: String
    let value: String?          // non-nil = revealed
    let onReveal: () -> Void
    let onCopy: () -> Void
    let onDelete: () -> Void
    @State private var hover = false

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "key.fill").font(.system(size: 10)).foregroundStyle(Theme.dim)
            Text("{{secret.\(name)}}").font(Theme.mono(12)).foregroundStyle(Theme.tool).lineLimit(1)
            Spacer(minLength: 12)
            if let v = value {
                Text(v.isEmpty ? "(empty)" : v)
                    .font(Theme.mono(11)).foregroundStyle(Theme.fgMuted)
                    .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                    .frame(maxWidth: 260, alignment: .trailing)
                    .padding(.horizontal, 8).padding(.vertical, 3)
                    .background(Theme.bg, in: RoundedRectangle(cornerRadius: 5))
                    .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Theme.borderSoft))
            }
            HStack(spacing: 4) {
                iconBtn(value != nil ? "eye.slash" : "eye", Theme.fgMuted, "Reveal value", onReveal)
                iconBtn("doc.on.doc", Theme.fgMuted, "Copy value", onCopy)
                iconBtn("trash", Theme.danger, "Delete secret", onDelete)
            }
            .opacity(hover ? 1 : 0.5)
        }
        .padding(.horizontal, 16).padding(.vertical, 8)
        .background(hover ? Theme.hover : .clear)
        .onHover { hover = $0 }
    }

    private func iconBtn(_ sys: String, _ color: Color, _ tip: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: sys).font(.system(size: 11)).frame(width: 22, height: 20)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain).foregroundStyle(color).help(tip)
    }
}
