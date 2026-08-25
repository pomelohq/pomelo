import SwiftUI
import AppKit

struct HoverRowModifier: ViewModifier {
    @Binding var hover: Bool
    var cursor: Bool = true
    @State private var pushed = false
    func body(content: Content) -> some View {
        content.onHover { inside in
            hover = inside
            let want = inside && cursor
            if want && !pushed { NSCursor.pointingHand.push(); pushed = true }
            else if !want && pushed { NSCursor.pop(); pushed = false }
        }
        .onDisappear { if pushed { NSCursor.pop(); pushed = false } }
    }
}

extension View {
    func hoverRow(_ hover: Binding<Bool>, cursor: Bool = true) -> some View {
        modifier(HoverRowModifier(hover: hover, cursor: cursor))
    }

    func dropdownMenu<M: View>(isPresented: Binding<Bool>, alignment: Alignment = .topLeading,
                               drop: CGFloat = 28, @ViewBuilder menu: @escaping () -> M) -> some View {
        overlay(alignment: alignment) {
            if isPresented.wrappedValue {
                ZStack(alignment: alignment) {
                    Color.black.opacity(0.001)
                        .frame(width: 6000, height: 6000)
                        .contentShape(Rectangle())
                        .onTapGesture { isPresented.wrappedValue = false }
                    menu().fixedSize()
                        .padding(.top, drop)
                        .shadow(color: .black.opacity(0.35), radius: 12, y: 4)
                }
            }
        }
        .zIndex(isPresented.wrappedValue ? 100 : 0)
    }
}

struct RightClickCatcher: NSViewRepresentable {
    let onRightClick: (CGPoint) -> Void
    func makeNSView(context: Context) -> NSView { RCView(onRightClick) }
    func updateNSView(_ v: NSView, context: Context) {}

    final class RCView: NSView {
        let cb: (CGPoint) -> Void
        init(_ cb: @escaping (CGPoint) -> Void) { self.cb = cb; super.init(frame: .zero) }
        required init?(coder: NSCoder) { fatalError() }
        override func hitTest(_ point: NSPoint) -> NSView? {
            if let e = NSApp.currentEvent, e.type == .rightMouseDown || e.type == .rightMouseUp {
                return self
            }
            return nil
        }
        override func rightMouseDown(with event: NSEvent) {
            let loc = event.locationInWindow
            let h = window?.contentView?.bounds.height ?? 0
            cb(CGPoint(x: loc.x, y: h - loc.y))
        }
    }
}

struct ChipSelect: View {
    let text: String
    let color: Color
    let options: [String]
    var current: String? = nil
    let onPick: (String) -> Void
    @State private var hover = false
    @State private var open = false

    var body: some View {
        Button { open.toggle() } label: {
            HStack(spacing: 3) {
                Text(text).font(Theme.mono(11))
                Image(systemName: "chevron.down").font(.system(size: 6, weight: .bold)).opacity(0.7)
            }
            .foregroundStyle(color)
            .padding(.horizontal, 8).padding(.vertical, 2.5)
            .background(color.opacity(hover ? 0.18 : 0.10), in: Capsule())
            .overlay(Capsule().strokeBorder(color.opacity(0.35)))
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .popover(isPresented: $open, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 1) {
                ForEach(options, id: \.self) { opt in
                    ChipSelectRow(title: opt, selected: opt == (current ?? text), tint: color) {
                        onPick(opt); open = false
                    }
                }
            }
            .padding(5)
            .frame(minWidth: 180)
            .background(Theme.panel3)
        }
    }
}

private struct ChipSelectRow: View {
    let title: String
    let selected: Bool
    let tint: Color
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(tint).opacity(selected ? 1 : 0).frame(width: 12)
                Text(title).font(Theme.mono(11.5)).foregroundStyle(selected ? tint : Theme.fg)
                Spacer(minLength: 12)
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(hover ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

struct PopItem: View {
    let title: String
    var icon: String? = nil
    var checked = false
    var destructive = false
    var tint: Color = Theme.accent
    let action: () -> Void
    @State private var hover = false

    init(_ title: String, icon: String? = nil, checked: Bool = false, destructive: Bool = false,
         tint: Color = Theme.accent, action: @escaping () -> Void) {
        self.title = title; self.icon = icon; self.checked = checked
        self.destructive = destructive; self.tint = tint; self.action = action
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                if let icon { Image(systemName: icon).font(.system(size: 10)).frame(width: 14).foregroundStyle(destructive ? Theme.danger : Theme.fgMuted) }
                Text(title).font(.system(size: 12)).foregroundStyle(destructive ? Theme.danger : Theme.fg).lineLimit(1)
                Spacer(minLength: 14)
                if checked { Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundStyle(tint) }
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(hover ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

struct PopMenu<Content: View>: View {
    var systemImage = "ellipsis"
    @ViewBuilder let content: (_ close: @escaping () -> Void) -> Content
    @State private var open = false
    @State private var hover = false

    var body: some View {
        Button { open.toggle() } label: {
            Image(systemName: systemImage).font(.system(size: 12))
                .foregroundStyle(hover || open ? Theme.fg : Theme.dim)
                .frame(width: 24, height: 22)
                .background(hover || open ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 5))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .dropdownMenu(isPresented: $open, alignment: .topTrailing, drop: 26) {
            VStack(spacing: 1) { content { open = false } }
                .padding(5).frame(minWidth: 160)
                .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border))
        }
    }
}
