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

final class ZoomControl: UIView {
    struct Stop { let display: CGFloat; let device: CGFloat; let short: String }

    var onSelect: ((CGFloat) -> Void)?
    private var stops: [Stop] = []
    private var buttons: [UIButton] = []
    private let stack = UIStackView()

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = UIColor.black.withAlphaComponent(0.4)
        layer.cornerRadius = 20
        stack.axis = .horizontal
        stack.alignment = .center
        stack.spacing = 2
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func configure(_ stops: [Stop]) {
        self.stops = stops
        buttons.forEach { $0.removeFromSuperview() }
        buttons = stops.enumerated().map { i, s in
            let b = UIButton(type: .system)
            b.setTitle(s.short, for: .normal)
            b.tag = i
            b.addTarget(self, action: #selector(tap(_:)), for: .touchUpInside)
            b.widthAnchor.constraint(greaterThanOrEqualToConstant: 34).isActive = true
            stack.addArrangedSubview(b)
            return b
        }
    }

    @objc private func tap(_ b: UIButton) { onSelect?(stops[b.tag].device) }

    func update(deviceZoom: CGFloat, pivot: CGFloat) {
        guard !stops.isEmpty else { return }
        var active = 0
        for (i, s) in stops.enumerated() where deviceZoom + 0.01 >= s.device { active = i }
        let display = deviceZoom / pivot
        for (i, b) in buttons.enumerated() {
            if i == active {
                let txt = abs(display - display.rounded()) < 0.05
                    ? String(format: "%.0fx", display) : String(format: "%.1fx", display)
                b.setTitle(txt, for: .normal)
                b.setTitleColor(.systemYellow, for: .normal)
                b.titleLabel?.font = .systemFont(ofSize: 14, weight: .bold)
            } else {
                b.setTitle(stops[i].short, for: .normal)
                b.setTitleColor(.white, for: .normal)
                b.titleLabel?.font = .systemFont(ofSize: 12, weight: .semibold)
            }
        }
    }
}

final class ScannerVC: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Void)?
    private let session = AVCaptureSession()
    private var lastCode = ""
    private var device: AVCaptureDevice?
    private var preview: AVCaptureVideoPreviewLayer?
    private let zoomBar = ZoomControl()
    private var zoomPivot: CGFloat = 1
    private var zoomMax: CGFloat = 8

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        // The Pro wide lens has too long a minimum focus distance to focus a QR held
        // close; a dual/triple virtual camera auto-switches to the ultra-wide (which
        // focuses much nearer) for close subjects.
        guard let device = AVCaptureDevice.default(.builtInTripleCamera, for: .video, position: .back)
                ?? AVCaptureDevice.default(.builtInDualWideCamera, for: .video, position: .back)
                ?? AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back)
                ?? AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return }
        self.device = device
        session.addInput(input)
        configureFocus(device)

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        self.preview = preview

        view.addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(focusTap(_:))))
        view.addGestureRecognizer(UIPinchGestureRecognizer(target: self, action: #selector(zoomPinch(_:))))

        setupZoomBar(device)

        Task.detached { [session] in session.startRunning() }
    }

    private func setupZoomBar(_ device: AVCaptureDevice) {
        let hasUW = device.deviceType == .builtInDualWideCamera || device.deviceType == .builtInTripleCamera
        let switchOvers = device.virtualDeviceSwitchOverVideoZoomFactors.map { CGFloat(truncating: $0) }
        // Device zoom factor is relative to the ultra-wide; the first switch-over point
        // is where "1x" (the wide lens) begins, so it maps device zoom to displayed zoom.
        let pivot = hasUW ? (switchOvers.first ?? 2) : 1
        zoomPivot = pivot
        zoomMax = min(device.activeFormat.videoMaxZoomFactor, max(pivot * 8, 8))

        var stops: [ZoomControl.Stop] = []
        if hasUW { stops.append(.init(display: 1 / pivot, device: 1, short: "0.5")) }
        stops.append(.init(display: 1, device: pivot, short: "1"))
        if hasUW, switchOvers.count >= 2 {
            let tele = switchOvers[1]
            stops.append(.init(display: tele / pivot, device: tele, short: shortLabel(tele / pivot)))
        } else {
            stops.append(.init(display: 2, device: pivot * 2, short: "2"))
        }
        zoomBar.configure(stops)
        zoomBar.onSelect = { [weak self] dev in self?.setZoom(dev, animated: true) }

        zoomBar.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(zoomBar)
        NSLayoutConstraint.activate([
            zoomBar.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            zoomBar.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -16),
            zoomBar.heightAnchor.constraint(equalToConstant: 40),
        ])

        setZoom(pivot, animated: false)
    }

    private func setZoom(_ deviceZoom: CGFloat, animated: Bool) {
        guard let device else { return }
        let target = max(1, min(deviceZoom, zoomMax))
        if (try? device.lockForConfiguration()) != nil {
            if animated { device.ramp(toVideoZoomFactor: target, withRate: 12) }
            else { device.videoZoomFactor = target }
            device.unlockForConfiguration()
        }
        zoomBar.update(deviceZoom: target, pivot: zoomPivot)
    }

    private func shortLabel(_ v: CGFloat) -> String {
        abs(v - v.rounded()) < 0.05 ? String(format: "%.0f", v) : String(format: "%.1f", v)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.layer.bounds
    }

    private func configureFocus(_ device: AVCaptureDevice) {
        guard (try? device.lockForConfiguration()) != nil else { return }
        if device.isFocusModeSupported(.continuousAutoFocus) { device.focusMode = .continuousAutoFocus }
        // QR is scanned up close; bias the lens to the near range so it locks faster.
        if device.isAutoFocusRangeRestrictionSupported { device.autoFocusRangeRestriction = .near }
        if device.isSmoothAutoFocusSupported { device.isSmoothAutoFocusEnabled = true }
        device.unlockForConfiguration()
    }

    @objc private func focusTap(_ g: UITapGestureRecognizer) {
        guard let device, let preview else { return }
        let loc = g.location(in: view)
        showFocusIndicator(at: loc)
        let pt = preview.captureDevicePointConverted(fromLayerPoint: loc)
        guard (try? device.lockForConfiguration()) != nil else { return }
        if device.isFocusPointOfInterestSupported, device.isFocusModeSupported(.autoFocus) {
            device.focusPointOfInterest = pt
            device.focusMode = .autoFocus
        }
        if device.isExposurePointOfInterestSupported, device.isExposureModeSupported(.continuousAutoExposure) {
            device.exposurePointOfInterest = pt
            device.exposureMode = .continuousAutoExposure
        }
        device.unlockForConfiguration()
    }

    private var zoomBase: CGFloat = 1

    @objc private func zoomPinch(_ g: UIPinchGestureRecognizer) {
        guard let device else { return }
        if g.state == .began { zoomBase = device.videoZoomFactor }
        setZoom(zoomBase * g.scale, animated: false)
    }

    private func showFocusIndicator(at point: CGPoint) {
        let box = UIView(frame: CGRect(x: 0, y: 0, width: 76, height: 76))
        box.center = point
        box.layer.borderColor = UIColor.systemYellow.cgColor
        box.layer.borderWidth = 1.5
        box.layer.cornerRadius = 5
        box.backgroundColor = .clear
        box.alpha = 0
        box.transform = CGAffineTransform(scaleX: 1.3, y: 1.3)
        view.addSubview(box)
        UIView.animate(withDuration: 0.15, animations: {
            box.alpha = 1
            box.transform = .identity
        }, completion: { _ in
            UIView.animate(withDuration: 0.4, delay: 0.5, animations: { box.alpha = 0 },
                           completion: { _ in box.removeFromSuperview() })
        })
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
