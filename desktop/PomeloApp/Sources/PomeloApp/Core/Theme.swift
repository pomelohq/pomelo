import SwiftUI
import AppKit

extension Color {
    init(hex: UInt32, alpha: Double = 1) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xff) / 255,
                  green: Double((hex >> 8) & 0xff) / 255,
                  blue: Double(hex & 0xff) / 255,
                  opacity: alpha)
    }
    static func w(_ a: Double) -> Color { Color(white: 1, opacity: a) }
}

enum ThemeMode: String, CaseIterable {
    case dark, light, sepia
    var isDark: Bool { self != .light }
}

/// How read-only code views handle a line wider than the pane. A preference
/// because it is a genuine split: wrapping keeps everything on screen, scrolling
/// keeps the split diff's two columns aligned line-for-line.
enum CodeWrapMode: String, CaseIterable {
    case wrap, scroll
    var wraps: Bool { self == .wrap }
    var label: String { self == .wrap ? "Wrap" : "Scroll" }
}

// Read by AppKit render code, which has no view context to inject through.
var activeCodeWrapMode: CodeWrapMode =
    CodeWrapMode(rawValue: UserDefaults.standard.string(forKey: "codeWrapMode") ?? "scroll") ?? .scroll

final class CodeDisplayManager: ObservableObject {
    static let shared = CodeDisplayManager()
    @Published var wrapMode: CodeWrapMode = activeCodeWrapMode {
        didSet {
            activeCodeWrapMode = wrapMode
            UserDefaults.standard.set(wrapMode.rawValue, forKey: "codeWrapMode")
        }
    }
    @Published var defaultSplit: Bool = UserDefaults.standard.bool(forKey: "diffDefaultSplit") {
        didSet { UserDefaults.standard.set(defaultSplit, forKey: "diffDefaultSplit") }
    }
}

struct Palette {
    let bg, bgSoft, surface, panel3: Color
    let borderSoft, border, borderHi: Color
    let fg, fgSoft, fgMuted, muted, dim: Color
    let accent, accentSoft, sel: Color
    let danger, dangerSoft, warn, warnSoft, ok, tool, wsAccent: Color
    let chip, chipBd, hover: Color
}

var activeThemeMode: ThemeMode = ThemeMode(rawValue: UserDefaults.standard.string(forKey: "themeMode") ?? "dark") ?? .dark

@MainActor
final class ThemeManager: ObservableObject {
    @Published var mode: ThemeMode = activeThemeMode {
        didSet {
            activeThemeMode = mode
            UserDefaults.standard.set(mode.rawValue, forKey: "themeMode")
            NSApp.appearance = NSAppearance(named: mode.isDark ? .darkAqua : .aqua)
        }
    }
    func cycle() {
        let all = ThemeMode.allCases
        mode = all[(all.firstIndex(of: mode)! + 1) % all.count]
    }
    func applyToWindow() { NSApp.appearance = NSAppearance(named: mode.isDark ? .darkAqua : .aqua) }
}

enum Theme {
    static var p: Palette {
        switch activeThemeMode {
        case .dark:  return dark
        case .light: return light
        case .sepia: return sepia
        }
    }
    static var bg: Color { p.bg }
    static var bgSoft: Color { p.bgSoft }
    static var surface: Color { p.surface }
    static var panel3: Color { p.panel3 }
    static var borderSoft: Color { p.borderSoft }
    static var border: Color { p.border }
    static var borderHi: Color { p.borderHi }
    static var fg: Color { p.fg }
    static var fgSoft: Color { p.fgSoft }
    static var fgMuted: Color { p.fgMuted }
    static var muted: Color { p.muted }
    static var dim: Color { p.dim }
    static var accent: Color { p.accent }
    static var accentSoft: Color { p.accentSoft }
    static var sel: Color { p.sel }
    static var danger: Color { p.danger }
    static var dangerSoft: Color { p.dangerSoft }
    static var warn: Color { p.warn }
    static var warnSoft: Color { p.warnSoft }
    static var ok: Color { p.ok }
    static var tool: Color { p.tool }
    static var wsAccent: Color { p.wsAccent }
    static var chip: Color { p.chip }
    static var chipBd: Color { p.chipBd }
    static var hover: Color { p.hover }

    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font { .system(size: size, weight: weight, design: .monospaced) }
    static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font { .system(size: size, weight: weight) }

    static let dark = Palette(
        bg: Color(hex: 0x17171a), bgSoft: Color(hex: 0x202024), surface: Color(hex: 0x202024), panel3: Color(hex: 0x2e2e33),
        borderSoft: .w(0.08), border: .w(0.13), borderHi: .w(0.20),
        fg: .white, fgSoft: Color(hex: 0xebebf5, alpha: 0.86), fgMuted: Color(hex: 0xebebf5, alpha: 0.60), muted: Color(hex: 0xebebf5, alpha: 0.42), dim: Color(hex: 0xebebf5, alpha: 0.30),
        accent: Color(hex: 0x0a84ff), accentSoft: Color(hex: 0x0a84ff, alpha: 0.16), sel: Color(hex: 0x0a84ff, alpha: 0.24),
        danger: Color(hex: 0xff453a), dangerSoft: Color(hex: 0xff453a, alpha: 0.16), warn: Color(hex: 0xff9f0a), warnSoft: Color(hex: 0xff9f0a, alpha: 0.15),
        ok: Color(hex: 0x30d158), tool: Color(hex: 0x64d2ff), wsAccent: Color(hex: 0xbf5af2),
        chip: .w(0.07), chipBd: .w(0.10), hover: .w(0.07))

    static let light = Palette(
        bg: Color(hex: 0xf2f2f7), bgSoft: Color(hex: 0xffffff), surface: Color(hex: 0xffffff), panel3: Color(hex: 0xe5e5ea),
        borderSoft: Color(hex: 0x3c3c43, alpha: 0.12), border: Color(hex: 0x3c3c43, alpha: 0.22), borderHi: Color(hex: 0x3c3c43, alpha: 0.32),
        fg: Color(hex: 0x000000), fgSoft: Color(hex: 0x3c3c43, alpha: 0.90), fgMuted: Color(hex: 0x3c3c43, alpha: 0.60), muted: Color(hex: 0x3c3c43, alpha: 0.45), dim: Color(hex: 0x3c3c43, alpha: 0.30),
        accent: Color(hex: 0x007aff), accentSoft: Color(hex: 0x007aff, alpha: 0.14), sel: Color(hex: 0x007aff, alpha: 0.18),
        danger: Color(hex: 0xff3b30), dangerSoft: Color(hex: 0xff3b30, alpha: 0.12), warn: Color(hex: 0xff9500), warnSoft: Color(hex: 0xff9500, alpha: 0.14),
        ok: Color(hex: 0x34c759), tool: Color(hex: 0x0a84ff), wsAccent: Color(hex: 0xaf52de),
        chip: Color(hex: 0x000000, alpha: 0.05), chipBd: Color(hex: 0x000000, alpha: 0.10), hover: Color(hex: 0x000000, alpha: 0.05))

    static let sepia = Palette(
        bg: Color(hex: 0x201b14), bgSoft: Color(hex: 0x2a241b), surface: Color(hex: 0x2a241b), panel3: Color(hex: 0x352d22),
        borderSoft: Color(hex: 0xd9c7a3, alpha: 0.10), border: Color(hex: 0xd9c7a3, alpha: 0.16), borderHi: Color(hex: 0xd9c7a3, alpha: 0.26),
        fg: Color(hex: 0xf0e6d2), fgSoft: Color(hex: 0xf0e6d2, alpha: 0.86), fgMuted: Color(hex: 0xf0e6d2, alpha: 0.60), muted: Color(hex: 0xf0e6d2, alpha: 0.44), dim: Color(hex: 0xf0e6d2, alpha: 0.30),
        accent: Color(hex: 0xcc7a3b), accentSoft: Color(hex: 0xcc7a3b, alpha: 0.18), sel: Color(hex: 0xcc7a3b, alpha: 0.22),
        danger: Color(hex: 0xd9584b), dangerSoft: Color(hex: 0xd9584b, alpha: 0.16), warn: Color(hex: 0xd99a3b), warnSoft: Color(hex: 0xd99a3b, alpha: 0.16),
        ok: Color(hex: 0x8ba05a), tool: Color(hex: 0x7bb0b8), wsAccent: Color(hex: 0xa87ac2),
        chip: Color(hex: 0xf0e6d2, alpha: 0.06), chipBd: Color(hex: 0xf0e6d2, alpha: 0.10), hover: Color(hex: 0xf0e6d2, alpha: 0.06))
}
