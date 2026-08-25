import SwiftUI
import AppKit

struct CustomHeader: View {
    @EnvironmentObject var theme: ThemeManager
    @EnvironmentObject var state: AppState
    var body: some View {
        ZStack {
            WindowDragArea()
            if let ws = state.selectedWorkspace {
                Text(ws.title).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.fgMuted)
                    .lineLimit(1).allowsHitTesting(false)
                    .padding(.horizontal, 260)
            }
            HStack(spacing: 8) {
                HeaderLeading()
                Spacer(minLength: 12)
                HeaderTrailing()
            }
            .padding(.leading, state.fullscreen ? 14 : 92)
            .padding(.trailing, 10)
        }
        .frame(height: 38)
        .frame(maxWidth: .infinity)
        .background(Theme.bgSoft)
    }
}

struct WindowConfigurator: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let v = NSView()
        DispatchQueue.main.async {
            guard let w = v.window else { return }
            w.titleVisibility = .hidden
            w.titlebarAppearsTransparent = true
            w.contentView?.wantsLayer = true
        }
        return v
    }
    func updateNSView(_ v: NSView, context: Context) {}
}

struct WindowDragArea: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { DragNSView() }
    func updateNSView(_ v: NSView, context: Context) {}
    final class DragNSView: NSView {
        override func mouseDown(with event: NSEvent) {
            if event.clickCount == 2 {
                switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") ?? "Maximize" {
                case "Minimize": window?.miniaturize(nil)
                case "None": break
                default: window?.performZoom(nil)
                }
                return
            }
            window?.performDrag(with: event)
        }
    }
}

struct SessionAnchorKey: PreferenceKey {
    static let defaultValue: Anchor<CGRect>? = nil
    static func reduce(value: inout Anchor<CGRect>?, nextValue: () -> Anchor<CGRect>?) {
        value = value ?? nextValue()
    }
}

struct HeaderLeading: View {
    @EnvironmentObject var state: AppState

    var body: some View {
        HStack(spacing: 8) {
            Button { state.showSessions.toggle() } label: {
                HStack(spacing: 6) {
                    Image(nsImage: NSApplication.shared.applicationIconImage)
                        .resizable().frame(width: 16, height: 16)
                    Text(PomCore.shared.session.isEmpty ? "pomelo" : PomCore.shared.session)
                        .font(.system(size: 13, weight: .semibold))
                    Image(systemName: "chevron.down").font(.system(size: 8, weight: .bold)).foregroundStyle(.secondary)
                }
            }
            .buttonStyle(.plain)
            .anchorPreference(key: SessionAnchorKey.self, value: .bounds) { $0 }
            .task { await state.loadSessions() }
        }
        .fixedSize()
    }
}

struct SessionsMenu: View {
    @EnvironmentObject var state: AppState
    @State private var confirmDeleteSession: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            SessionMenuButton(label: "New session…", icon: "plus.circle") {
                state.showSessions = false; state.showCreateSession = true
            }
            SessionMenuButton(label: "Open a session…", icon: "folder") {
                state.showSessions = false; state.openExistingSession()
            }
            Divider().overlay(Theme.borderSoft).padding(.vertical, 4)
            Text("SESSIONS").font(.system(size: 10, weight: .semibold)).kerning(0.6)
                .foregroundStyle(Theme.muted).padding(.horizontal, 10).padding(.bottom, 4)
            ForEach(state.sessions) { s in
                SessionRow(session: s,
                           onSwitch: { state.switchSession(s.name); state.showSessions = false },
                           onDelete: { confirmDeleteSession = s.name })
            }
        }
        .frame(width: 258).padding(.top, 6).padding(.bottom, 6)
        .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.border))
        .shadow(color: .black.opacity(0.35), radius: 14, y: 5)
        .confirmationDialog("Delete session \(confirmDeleteSession ?? "")?", isPresented: Binding(get: { confirmDeleteSession != nil }, set: { if !$0 { confirmDeleteSession = nil } }), titleVisibility: .visible) {
            Button("Delete", role: .destructive) { if let n = confirmDeleteSession { state.deleteSession(n) }; confirmDeleteSession = nil }
            Button("Cancel", role: .cancel) { confirmDeleteSession = nil }
        } message: { Text("Removes it from the session list. Files on disk are left untouched.") }
    }
}

private struct SessionMenuButton: View {
    let label: String
    let icon: String
    let action: () -> Void
    @State private var hover = false
    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: icon).font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                Text(label).font(.system(size: 12.5)).foregroundStyle(Theme.fg)
                Spacer()
            }
            .padding(.horizontal, 10).padding(.vertical, 5)
            .background(hover ? Theme.hover : .clear, in: RoundedRectangle(cornerRadius: 5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .hoverRow($hover)
    }
}

private struct SessionRow: View {
    let session: SessionItem
    let onSwitch: () -> Void
    let onDelete: () -> Void
    @State private var hover = false
    private var dead: Bool { !session.isAvailable }
    private var selectable: Bool { session.isAvailable && !session.current }

    var body: some View {
        Button { if selectable { onSwitch() } } label: {
            HStack(spacing: 8) {
                Circle().fill(session.current ? Theme.ok : Theme.dim).frame(width: 6, height: 6)
                Text(session.name).font(.system(size: 12.5))
                    .foregroundStyle(dead ? Theme.dim : Theme.fg).strikethrough(dead)
                Spacer(minLength: 12)
                if session.current {
                    Text("CURRENT").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.ok)
                        .padding(.horizontal, 5).padding(.vertical, 1).background(Theme.ok.opacity(0.15), in: Capsule())
                } else if dead {
                    Text("missing").font(.system(size: 9, weight: .medium)).foregroundStyle(Theme.dim)
                }
            }
            .padding(.horizontal, 10).padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(session.current ? Theme.sel : (hover && selectable ? Theme.hover : .clear), in: RoundedRectangle(cornerRadius: 5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!selectable)
        .hoverRow($hover, cursor: selectable)
        .overlay(alignment: .trailing) {
            if !session.current && (dead || hover) {
                Button(role: .destructive, action: onDelete) { Image(systemName: "trash").font(.system(size: 9)) }
                    .buttonStyle(.plain).foregroundStyle(dead ? Theme.warn : Theme.dim).padding(.trailing, 12)
            }
        }
    }
}

struct HeaderTrailing: View {
    @EnvironmentObject var theme: ThemeManager
    @EnvironmentObject var state: AppState
    private var icon: String {
        switch theme.mode { case .dark: return "moon.fill"; case .light: return "sun.max.fill"; case .sepia: return "book.fill" }
    }
    var body: some View {
        HStack(spacing: 12) {
            if state.agentModel != nil { agentChip }
            Button { state.showShared = true } label: { Image(systemName: "cylinder.split.1x2").font(.system(size: 12)) }
                .buttonStyle(.plain).tooltip("Shared services", shortcut: "⇧⌘S", align: .bottomTrailing)
            Button { state.openActivity(scope: nil) } label: { Image(systemName: "gauge.with.dots.needle.67percent").font(.system(size: 12)) }
                .buttonStyle(.plain).tooltip("Activity Monitor · all workspaces", shortcut: "⇧⌘0", align: .bottomTrailing)
            Button { state.showSessionPanel = true } label: { Image(systemName: "chevron.left.forwardslash.chevron.right").font(.system(size: 12)) }
                .buttonStyle(.plain).tooltip("Session — config editor + ENV inspector", shortcut: "⇧⌘P", align: .bottomTrailing)
            Button { theme.cycle() } label: { Image(systemName: icon).font(.system(size: 12)) }
                .buttonStyle(.plain).tooltip("Theme: \(theme.mode.rawValue)", shortcut: "⇧⌘T", align: .bottomTrailing)
            Button { state.showSettings = true } label: { Image(systemName: "gearshape").font(.system(size: 12)) }
                .buttonStyle(.plain).tooltip("Settings", shortcut: "⌘,", align: .bottomTrailing)
        }
        .frame(height: 28)
    }

    private var agentChip: some View {
        HStack(spacing: 5) {
            if state.agentRunning { ProgressView().controlSize(.small).scaleEffect(0.55) }
            else { Image(systemName: "checkmark.seal.fill").font(.system(size: 10)).foregroundStyle(Theme.ok) }
            Text(state.agentRunning ? state.agentTitle + "…" : state.agentTitle + " done")
                .font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.fg).lineLimit(1)
            Button { state.endAgent() } label: { Image(systemName: "xmark").font(.system(size: 8, weight: .bold)) }
                .buttonStyle(.plain).foregroundStyle(Theme.dim).help("Stop the agent")
        }
        .padding(.horizontal, 8).padding(.vertical, 3)
        .background(Theme.wsAccent.opacity(0.15), in: Capsule())
        .overlay(Capsule().strokeBorder(Theme.wsAccent.opacity(0.35)))
        .contentShape(Capsule())
        .onTapGesture { state.reopenAgent() }
        .help(state.agentRunning ? "Agent running — click to view" : "Agent finished — click to view")
    }
}
