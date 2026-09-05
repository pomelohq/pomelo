import SwiftUI
import AppKit

struct FileImageView: View {
    let image: NSImage
    @State private var zoom: CGFloat?
    @State private var pan: CGSize = .zero
    @State private var fitScale: CGFloat = 1
    @GestureState private var drag: CGSize = .zero
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        VStack(spacing: 0) {
            stage
            Divider().overlay(Theme.borderSoft)
            zoomBar
        }
        .background(Theme.bg)
    }

    private var stage: some View {
        GeometryReader { geo in
            let fit = min(1, min((geo.size.width - 40) / image.size.width, (geo.size.height - 40) / image.size.height))
            let scale = (zoom ?? fit) * pinch
            let shown = CGSize(width: image.size.width * scale, height: image.size.height * scale)
            let slack = CGSize(width: max(0, shown.width - geo.size.width) / 2,
                               height: max(0, shown.height - geo.size.height) / 2)
            let offset = clampPan(CGSize(width: pan.width + drag.width, height: pan.height + drag.height), slack)
            Image(nsImage: image)
                .resizable().interpolation(.high)
                .frame(width: shown.width, height: shown.height)
                .offset(x: offset.width, y: offset.height)
                .frame(width: geo.size.width, height: geo.size.height)
                .clipped()
                .contentShape(Rectangle())
                .gesture(panGesture(slack: slack))
                .simultaneousGesture(MagnificationGesture().updating($pinch) { v, st, _ in st = v }
                    .onEnded { v in
                        zoom = clampZoom((zoom ?? fit) * v)
                        pan = clampPan(pan, slack)
                    })
                .onTapGesture(count: 2) {
                    withAnimation(.easeOut(duration: 0.15)) {
                        if zoom == nil { zoom = clampZoom(fit * 2) } else { reset() }
                    }
                }
                .background(ScrollWheelCatcher { delta in
                    zoom = clampZoom((zoom ?? fit) * (1 + delta * 0.008))
                    pan = clampPan(pan, slack)
                })
                .onAppear { fitScale = fit }
                .onChange(of: geo.size) { _ in fitScale = fit }
                .background(keys)
        }
    }

    private var keys: some View {
        Group {
            Button("") { zoomBy(1.25) }.keyboardShortcut("=", modifiers: .command)
            Button("") { zoomBy(0.8) }.keyboardShortcut("-", modifiers: .command)
            Button("") { withAnimation(.easeOut(duration: 0.12)) { reset() } }.keyboardShortcut("0", modifiers: .command)
        }.hidden()
    }

    private var zoomBar: some View {
        HStack(spacing: 4) {
            IconButton("minus.magnifyingglass", tip: "Zoom out (⌘−)") { zoomBy(0.8) }
            IconButton("plus.magnifyingglass", tip: "Zoom in (⌘+)") { zoomBy(1.25) }
            Button {
                withAnimation(.easeOut(duration: 0.12)) { reset() }
            } label: {
                Text("\(Int((((zoom ?? fitScale) * pinch) * 100).rounded()))%")
                    .font(Theme.mono(10.5)).foregroundStyle(Theme.fgMuted).monospacedDigit()
                    .frame(minWidth: 44)
            }
            .buttonStyle(.plain).help("Reset to fit (⌘0)")
            Spacer()
            Text("\(Int(image.size.width))×\(Int(image.size.height))")
                .font(Theme.mono(10.5)).foregroundStyle(Theme.dim)
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .background(Theme.bgSoft)
    }

    private func panGesture(slack: CGSize) -> some Gesture {
        DragGesture(minimumDistance: 2)
            .updating($drag) { v, st, _ in
                guard slack.width > 0 || slack.height > 0 else { return }
                st = v.translation
            }
            .onEnded { v in
                guard slack.width > 0 || slack.height > 0 else { return }
                pan = clampPan(CGSize(width: pan.width + v.translation.width,
                                      height: pan.height + v.translation.height), slack)
            }
    }

    private func clampPan(_ p: CGSize, _ slack: CGSize) -> CGSize {
        CGSize(width: min(slack.width, max(-slack.width, p.width)),
               height: min(slack.height, max(-slack.height, p.height)))
    }

    private func clampZoom(_ z: CGFloat) -> CGFloat { min(8, max(0.1, z)) }
    private func zoomBy(_ f: CGFloat) {
        withAnimation(.easeOut(duration: 0.12)) { zoom = clampZoom((zoom ?? fitScale) * f) }
    }
    private func reset() { zoom = nil; pan = .zero }
}

private struct ScrollWheelCatcher: NSViewRepresentable {
    let onScroll: (CGFloat) -> Void

    func makeNSView(context: Context) -> CatcherView {
        let v = CatcherView()
        v.onScroll = onScroll
        return v
    }

    func updateNSView(_ nsView: CatcherView, context: Context) { nsView.onScroll = onScroll }

    final class CatcherView: NSView {
        var onScroll: ((CGFloat) -> Void)?

        override func scrollWheel(with event: NSEvent) {
            guard event.modifierFlags.contains(.command) else {
                super.scrollWheel(with: event)
                return
            }
            onScroll?(event.scrollingDeltaY)
        }
    }
}
