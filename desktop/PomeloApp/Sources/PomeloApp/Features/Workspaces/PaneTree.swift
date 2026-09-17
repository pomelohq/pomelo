import SwiftUI

@MainActor
@Observable final class PaneNode: Identifiable {
    enum Axis { case horizontal, vertical }
    enum Kind {
        case leaf(Workbench)
        indirect case branch(axis: Axis, first: PaneNode, second: PaneNode, fraction: CGFloat)
    }

    let id: String
    var kind: Kind

    init(id: String, kind: Kind) { self.id = id; self.kind = kind }

    static func leaf(_ wb: Workbench, id: String) -> PaneNode { PaneNode(id: id, kind: .leaf(wb)) }

    var leafWorkbench: Workbench? { if case .leaf(let wb) = kind { return wb }; return nil }

    var leaves: [PaneNode] {
        switch kind {
        case .leaf: return [self]
        case .branch(_, let a, let b, _): return a.leaves + b.leaves
        }
    }

    var branches: [PaneNode] {
        switch kind {
        case .leaf: return []
        case .branch(_, let a, let b, _): return [self] + a.branches + b.branches
        }
    }

    func leaf(withID leafID: String) -> PaneNode? {
        switch kind {
        case .leaf: return id == leafID ? self : nil
        case .branch(_, let a, let b, _): return a.leaf(withID: leafID) ?? b.leaf(withID: leafID)
        }
    }

    func splitting(leafID: String, axis: Axis, newLeaf: PaneNode, newFirst: Bool, branchID: String) -> PaneNode {
        switch kind {
        case .leaf:
            guard id == leafID else { return self }
            let (first, second) = newFirst ? (newLeaf, self) : (self, newLeaf)
            return PaneNode(id: branchID, kind: .branch(axis: axis, first: first, second: second, fraction: 0.5))
        case .branch(let ax, let a, let b, let f):
            kind = .branch(axis: ax,
                           first: a.splitting(leafID: leafID, axis: axis, newLeaf: newLeaf, newFirst: newFirst, branchID: branchID),
                           second: b.splitting(leafID: leafID, axis: axis, newLeaf: newLeaf, newFirst: newFirst, branchID: branchID),
                           fraction: f)
            return self
        }
    }

    func removing(leafID: String) -> PaneNode? {
        switch kind {
        case .leaf:
            return id == leafID ? nil : self
        case .branch(let ax, let a, let b, let f):
            let na = a.removing(leafID: leafID)
            let nb = b.removing(leafID: leafID)
            if na == nil { return nb }
            if nb == nil { return na }
            kind = .branch(axis: ax, first: na!, second: nb!, fraction: f)
            return self
        }
    }

    func setFraction(_ frac: CGFloat) {
        if case .branch(let ax, let a, let b, _) = kind {
            kind = .branch(axis: ax, first: a, second: b, fraction: min(0.98, max(0.02, frac)))
        }
    }

    var structureSignature: String {
        switch kind {
        case .leaf: return id
        case .branch(let ax, let a, let b, _):
            return "(\(ax == .horizontal ? "h" : "v") \(a.structureSignature) \(b.structureSignature))"
        }
    }

    var maxSeq: Int {
        let mine = Int(id.split(separator: "-").last.map(String.init) ?? "") ?? 0
        switch kind {
        case .leaf: return mine
        case .branch(_, let a, let b, _): return max(mine, max(a.maxSeq, b.maxSeq))
        }
    }

    func encoded() -> PersistedPane {
        switch kind {
        case .leaf:
            return PersistedPane(id: id, axis: nil, fraction: nil, first: nil, second: nil)
        case .branch(let ax, let a, let b, let f):
            return PersistedPane(id: id, axis: ax == .horizontal ? "h" : "v", fraction: f,
                                 first: a.encoded(), second: b.encoded())
        }
    }

    static func decoded(_ p: PersistedPane, leaf: (String) -> Workbench) -> PaneNode {
        if let first = p.first, let second = p.second, let axis = p.axis {
            return PaneNode(id: p.id, kind: .branch(axis: axis == "h" ? .horizontal : .vertical,
                                                    first: decoded(first, leaf: leaf),
                                                    second: decoded(second, leaf: leaf),
                                                    fraction: p.fraction ?? 0.5))
        }
        return .leaf(leaf(p.id), id: p.id)
    }
}

final class PersistedPane: Codable {
    var id: String
    var axis: String?
    var fraction: CGFloat?
    var first: PersistedPane?
    var second: PersistedPane?
    init(id: String, axis: String?, fraction: CGFloat?, first: PersistedPane?, second: PersistedPane?) {
        self.id = id; self.axis = axis; self.fraction = fraction; self.first = first; self.second = second
    }
}
