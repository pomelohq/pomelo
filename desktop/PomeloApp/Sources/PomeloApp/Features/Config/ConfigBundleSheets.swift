import SwiftUI
import AppKit

private struct SheetHeader: View {
    let icon: String
    let title: String
    let onClose: () -> Void
    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: icon).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.accent)
            Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.fg)
            Spacer()
            Button { onClose() } label: {
                Image(systemName: "xmark").font(.system(size: 11, weight: .bold)).foregroundStyle(Theme.fgMuted)
                    .frame(width: 22, height: 22).background(Theme.panel3, in: Circle())
            }.buttonStyle(.plain)
        }
    }
}

struct ExportBundleSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var includeSecrets = false
    @State private var password = ""
    @State private var secretCount = 0
    @State private var status = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SheetHeader(icon: "square.and.arrow.up", title: "Export config") { dismiss() }
            Card(background: Theme.bg) {
                VStack(alignment: .leading, spacing: 10) {
                    Toggle("Include secrets (\(secretCount))", isOn: $includeSecrets).disabled(secretCount == 0)
                    if includeSecrets {
                        SecureField("Encryption password", text: $password).textFieldStyle(.roundedBorder)
                        Text("Secrets are AES-256-GCM encrypted into a .pombundle. Share the password separately.")
                            .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                    } else {
                        Text("Exports the merged config as plain YAML (no secrets).")
                            .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                    }
                }
                .padding(12)
            }
            HStack {
                if !status.isEmpty { Text(status).font(.system(size: 11)).foregroundStyle(Theme.fgMuted) }
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Export…") { export() }.buttonStyle(.borderedProminent).tint(Theme.accent)
                    .disabled(includeSecrets && password.isEmpty)
            }
        }
        .padding(18).frame(width: 460)
        .background(Theme.bgSoft)
        .onAppear { Task {
            let d = await BundleStore.secretNames()
            struct R: Decodable { var names: [String]? }
            secretCount = (PomJSON.decode(R.self, from: d))?.names?.count ?? 0
        } }
    }

    private func export() {
        status = "exporting…"
        let inc = includeSecrets, pw = password
        Task {
            let d = await BundleStore.export(includeSecrets: inc, password: pw)
            struct R: Decodable { var filename = ""; var data = "" }
            guard let r = PomJSON.decode(R.self, from: d), let bytes = Data(base64Encoded: r.data) else { status = "export failed"; return }
            let panel = NSSavePanel(); panel.nameFieldStringValue = r.filename
            panel.begin { resp in
                if resp == .OK, let url = panel.url { try? bytes.write(to: url); dismiss() }
                else { status = "" }
            }
        }
    }
}

struct ImportBundleSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(AppState.self) var state
    @State private var dataB64 = ""
    @State private var fileName = ""
    @State private var picking = false
    @State private var password = ""
    @State private var needPassword = false
    @State private var yaml = ""
    @State private var secretNames: [String] = []
    @State private var loaded = false
    @State private var writeConfig = true
    @State private var createSecrets = true
    @State private var status = ""
    @State private var isError = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            SheetHeader(icon: "square.and.arrow.down", title: "Import config") { dismiss() }
            if !loaded { pickPhase } else { previewPhase }
        }
        .padding(18).frame(width: loaded ? 560 : 460)
        .background(Theme.bgSoft)
    }

    private var pickPhase: some View {
        VStack(alignment: .leading, spacing: 12) {
            filePicker
            if needPassword {
                Text("Encrypted bundle — enter the password to unlock.")
                    .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                SecureField("Password", text: $password).textFieldStyle(.roundedBorder)
                    .onSubmit { if !password.isEmpty { read() } }
            } else {
                Text("Pick a pom.yml or an encrypted .pombundle to import.")
                    .font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
            }
            if !status.isEmpty { Text(status).font(.system(size: 11)).foregroundStyle(isError ? Theme.danger : Theme.fgMuted) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                if needPassword {
                    Button("Unlock") { read() }.buttonStyle(.borderedProminent).tint(Theme.accent).disabled(password.isEmpty)
                }
            }
        }
    }

    private var filePicker: some View {
        Button { pick() } label: {
            HStack(spacing: 10) {
                Image(systemName: fileName.isEmpty ? "doc.badge.plus" : "doc.text.fill")
                    .font(.system(size: 15)).foregroundStyle(fileName.isEmpty ? Theme.fgMuted : Theme.accent)
                VStack(alignment: .leading, spacing: 1) {
                    Text(fileName.isEmpty ? "Choose file…" : fileName)
                        .font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fg).lineLimit(1)
                    if !fileName.isEmpty {
                        Text("Click to pick a different file").font(.system(size: 10)).foregroundStyle(Theme.fgMuted)
                    }
                }
                Spacer()
                if picking { Spinner(size: 12) }
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.bg, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(fileName.isEmpty ? Theme.accent.opacity(0.5) : Theme.borderSoft))
        }
        .buttonStyle(.plain).disabled(picking)
    }

    private var previewPhase: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !fileName.isEmpty {
                HStack(spacing: 8) {
                    Image(systemName: "doc.text.fill").font(.system(size: 12)).foregroundStyle(Theme.accent)
                    Text(fileName).font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg)
                    Button("Change") { resetToPick() }.buttonStyle(.plain).font(.system(size: 11)).foregroundStyle(Theme.accent)
                }
            }
            Card(background: Theme.bg) {
                ScrollView { Text(yaml).font(Theme.mono(11)).foregroundStyle(Theme.fgMuted).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(8) }
                    .frame(height: 220)
            }
            if !secretNames.isEmpty {
                Text("Secrets: \(secretNames.joined(separator: ", "))").font(.system(size: 11)).foregroundStyle(Theme.fgMuted).lineLimit(2)
                Toggle("Recreate secrets", isOn: $createSecrets)
            }
            Toggle("Overwrite my config with this (kept as .bak, split into pom.d/)", isOn: $writeConfig)
            if !status.isEmpty { Text(status).font(.system(size: 11)).foregroundStyle(isError ? Theme.danger : Theme.ok) }
            HStack {
                Button("Adapt with Claude") { adapt() }.help("Save the source config into the session and open Claude (with the pom MCP) to merge it into yours")
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Apply") { apply() }.buttonStyle(.borderedProminent).tint(Theme.accent)
            }
        }
    }

    private func resetToPick() {
        loaded = false; needPassword = false; yaml = ""; secretNames = []
        dataB64 = ""; fileName = ""; password = ""; status = ""; isError = false
    }

    private func pick() {
        guard !picking else { return }
        picking = true
        let panel = NSOpenPanel(); panel.allowsMultipleSelection = false
        panel.canChooseFiles = true; panel.canChooseDirectories = false
        panel.begin { resp in
            defer { picking = false }
            guard resp == .OK, let url = panel.url else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            do {
                let raw = try Data(contentsOf: url)
                guard !raw.isEmpty else { status = "file is empty"; isError = true; return }
                fileName = url.lastPathComponent
                needPassword = false; password = ""; status = ""; isError = false
                dataB64 = raw.base64EncodedString(); read()
            } catch {
                status = "could not read file: \(error.localizedDescription)"; isError = true
            }
        }
    }

    private func read() {
        status = ""; isError = false
        let dta = dataB64, pw = password
        Task {
            let d = await BundleStore.read(dataB64: dta, password: pw)
            guard !d.isEmpty else { status = "no response from the core — is a project open?"; isError = true; return }
            struct R: Decodable { var encrypted: Bool?; var need_password: Bool?; var yaml: String?; var secret_names: [String]?; var error: String? }
            guard let r = PomJSON.decode(R.self, from: d) else {
                status = "unexpected response: \(String(decoding: d.prefix(140), as: UTF8.self))"; isError = true; return
            }
            if let e = r.error, !e.isEmpty { status = e; isError = true; return }
            if r.need_password == true { needPassword = true; return }
            yaml = r.yaml ?? ""; secretNames = r.secret_names ?? []; loaded = true
        }
    }

    private func apply() {
        let dta = dataB64, pw = password, y = yaml, wc = writeConfig, cs = createSecrets
        Task {
            let d = await BundleStore.apply(dataB64: dta, password: pw, yaml: y, writeConfig: wc, createSecrets: cs)
            struct R: Decodable { var ok: Bool?; var secrets_created: Int?; var split: Bool?; var error: String? }
            if let r = PomJSON.decode(R.self, from: d), r.ok == true {
                let n = r.secrets_created ?? 0
                let bits = [r.split == true ? "split into pom.d/" : nil, n > 0 ? "\(n) secrets" : nil].compactMap { $0 }
                status = "Applied" + (bits.isEmpty ? "." : " · " + bits.joined(separator: " · ")); isError = false
                try? await Task.sleep(nanoseconds: 900_000_000); dismiss()
            } else {
                status = "apply failed: \(String(decoding: d.prefix(140), as: UTF8.self))"; isError = true
            }
        }
    }

    private func adapt() {
        let dta = dataB64, pw = password, y = yaml, cs = createSecrets
        Task {
            let d = await BundleStore.adapt(dataB64: dta, password: pw, yaml: y, createSecrets: cs)
            struct R: Decodable { var ok: Bool?; var prompt: String?; var error: String? }
            guard let r = PomJSON.decode(R.self, from: d), r.ok == true, let p = r.prompt, !p.isEmpty else {
                status = "adapt failed: \(String(decoding: d.prefix(140), as: UTF8.self))"; isError = true; return
            }
            state.launchAgent(title: "Adapt",
                              subtitle: "Claude merges the imported config into yours via the pom MCP tools, then validates.",
                              runningLabel: "Merging the imported config into your project…",
                              branch: "", role: "fixer", firstTurn: p)
            dismiss()
        }
    }
}
