import SwiftUI
import AVFoundation

struct PairingView: View {
    let onPaired: (PairedDevice) -> Void
    @EnvironmentObject var theme: ThemeManager
    @Environment(\.dismiss) private var dismiss
    @State private var manual = ""
    @State private var error = ""
    @State private var draft: PairedDevice?
    @State private var hostOptions: [String] = []

    var body: some View {
        NavigationStack {
            Group {
                if draft != nil { confirmForm } else { scanStep }
            }
            .background(Theme.bg.ignoresSafeArea())
            .navigationTitle(draft == nil ? "Pair a Mac" : "Confirm address")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSoft, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(draft == nil ? "Cancel" : "Back") { if draft != nil { draft = nil } else { dismiss() } }
                }
                if draft != nil {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Add") { if let d = draft { onPaired(d) } }
                            .disabled((draft?.host ?? "").isEmpty)
                    }
                }
            }
        }
        .tint(Theme.accent)
        .preferredColorScheme(activeThemeMode == .light ? .light : .dark)
    }

    private func decode(_ code: String) {
        if let r = PairedDevice.parse(code) {
            draft = r.device
            hostOptions = r.hosts
            error = ""
        } else { error = "That isn't a Pomelo pairing code." }
    }

    private var scanStep: some View {
        VStack(spacing: 0) {
            QRScanner { code in decode(code) }
                .overlay(alignment: .bottom) {
                    if !error.isEmpty {
                        Text(error).font(.caption).padding(8)
                            .background(.red.opacity(0.85), in: Capsule()).foregroundStyle(.white)
                            .padding(.bottom, 12)
                    }
                }
            Divider().overlay(Theme.borderSoft)
            VStack(alignment: .leading, spacing: 8) {
                Text("Or paste the pairing code").font(Theme.ui(11)).foregroundStyle(Theme.fgMuted)
                HStack {
                    TextField("Paste pairing code...", text: $manual, axis: .vertical)
                        .font(Theme.mono(12)).lineLimit(1...3)
                        .padding(8)
                        .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 8))
                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Theme.borderSoft, lineWidth: 1))
                    Button("Pair") { decode(manual) }
                        .buttonStyle(.borderedProminent).tint(Theme.accent)
                        .disabled(manual.isEmpty)
                }
            }
            .padding()
            .background(Theme.bg)
        }
    }

    @ViewBuilder private var confirmForm: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                SectionLabel(text: "Address")
                VStack(spacing: 0) {
                    ForEach(hostOptions, id: \.self) { h in
                        Button { draft?.host = h } label: {
                            HStack(spacing: 10) {
                                Image(systemName: (draft?.host == h) ? "largecircle.fill.circle" : "circle")
                                    .foregroundStyle((draft?.host == h) ? Theme.accent : Theme.dim)
                                Text(h).font(Theme.mono(13)).foregroundStyle(Theme.fg)
                                if h.hasPrefix("100.") {
                                    Text("Tailscale").font(Theme.ui(9, .medium)).foregroundStyle(Theme.tool)
                                        .padding(.horizontal, 6).padding(.vertical, 1).background(Theme.tool.opacity(0.15), in: Capsule())
                                }
                                Spacer()
                            }
                            .padding(.horizontal, 12).padding(.vertical, 11).contentShape(Rectangle())
                        }.buttonStyle(.plain)
                        if h != hostOptions.last { Divider().overlay(Theme.borderSoft) }
                    }
                }
                .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))

                SectionLabel(text: "Or enter a host manually")
                TextField("100.x.y.z or 192.168.x.x", text: Binding(
                    get: { draft?.host ?? "" }, set: { draft?.host = $0 }))
                    .font(Theme.mono(13)).foregroundStyle(Theme.fg)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                    .padding(10)
                    .background(Theme.bgSoft, in: RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(Theme.borderSoft, lineWidth: 1))

                Text("Pick the Tailscale IP (100.x) to reach this Mac from anywhere on your tailnet, or a LAN IP at home. Port \(draft?.port ?? "") · pinned cert.")
                    .font(Theme.ui(10.5)).foregroundStyle(Theme.fgMuted)
            }
            .padding(16)
        }
    }
}

struct QRScanner: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerVC {
        let vc = ScannerVC()
        vc.onCode = onCode
        return vc
    }
    func updateUIViewController(_ vc: ScannerVC, context: Context) {}
}

final class ScannerVC: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Void)?
    private let session = AVCaptureSession()
    private var lastCode = ""

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return }
        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        preview.frame = view.layer.bounds
        view.layer.addSublayer(preview)

        Task.detached { [session] in session.startRunning() }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        session.stopRunning()
    }

    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject],
                        from connection: AVCaptureConnection) {
        guard let obj = objects.first as? AVMetadataMachineReadableCodeObject,
              let s = obj.stringValue, s != lastCode else { return }
        lastCode = s
        onCode?(s)
    }
}
