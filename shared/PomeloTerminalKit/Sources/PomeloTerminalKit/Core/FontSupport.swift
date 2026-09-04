import CoreText
import CoreGraphics
#if os(macOS)
import AppKit
#else
import UIKit
#endif

public enum TerminalFonts {
    // Resolve a monospace CTFont for the given family (empty = the system monospaced
    // font). Uses CoreText for the family lookup so it works identically on macOS/iOS.
    public static func monoFont(_ size: CGFloat, family: String) -> CTFont {
        if !family.isEmpty {
            let desc = CTFontDescriptorCreateWithAttributes([kCTFontFamilyNameAttribute: family as CFString] as CFDictionary)
            let f = CTFontCreateWithFontDescriptor(desc, size, nil)
            if (CTFontCopyFamilyName(f) as String).caseInsensitiveCompare(family) == .orderedSame {
                return f
            }
        }
        return systemMono(size)
    }

    static func systemMono(_ size: CGFloat) -> CTFont {
        #if os(macOS)
        return NSFont.monospacedSystemFont(ofSize: size, weight: .regular) as CTFont
        #else
        return UIFont.monospacedSystemFont(ofSize: size, weight: .regular) as CTFont
        #endif
    }

    // Families for a font picker: monospace fonts, plus every Nerd Font (the non-"Mono"
    // Nerd variants aren't flagged monospace but are the ones people run in a terminal
    // for full icon coverage).
    public static func monospaceFamilies() -> [String] {
        let coll = CTFontCollectionCreateFromAvailableFonts(nil)
        let all = (CTFontCollectionCreateMatchingFontDescriptors(coll) as? [CTFontDescriptor]) ?? []
        var out = Set<String>()
        for d in all {
            guard let fam = CTFontDescriptorCopyAttribute(d, kCTFontFamilyNameAttribute) as? String, !fam.hasPrefix(".") else { continue }
            if fam.lowercased().contains("nerd font") { out.insert(fam); continue }
            let traits = (CTFontDescriptorCopyAttribute(d, kCTFontTraitsAttribute) as? [CFString: Any]) ?? [:]
            let sym = (traits[kCTFontSymbolicTrait] as? UInt32) ?? 0
            if sym & CTFontSymbolicTraits.traitMonoSpace.rawValue != 0 { out.insert(fam) }
        }
        return out.sorted()
    }
}
