import SwiftUI
import AppKit
import Combine

// The database navigator as a native NSOutlineView: NSTableView-backed row reuse
// (no LazyVStack stutter / scrollbar jump), native disclosure + indentation, and
// native top-level drag reorder that works regardless of what is expanded. AppKit
// is used here purely as a virtualization primitive — cells are drawn to our theme.

// Stable identity for outline items so expansion survives reloadData.
final class DBItem: NSObject {
    enum Kind { case backend(String); case db(String, String); case table(String, String) } // db/table carry (repoOrDbID, id)
    let key: String
    let kind: Kind
    init(key: String, kind: Kind) { self.key = key; self.kind = kind }
    override func isEqual(_ object: Any?) -> Bool { (object as? DBItem)?.key == key }
    override var hash: Int { key.hashValue }
}

struct DBTree: NSViewRepresentable {
    @ObservedObject var vm: DatabaseViewModel
    @ObservedObject var theme: ThemeManager

    func makeCoordinator() -> Coordinator { Coordinator(vm: vm, theme: theme) }

    func makeNSView(context: Context) -> NSScrollView {
        let outline = NSOutlineView()
        outline.headerView = nil
        outline.backgroundColor = NSColor(Theme.bgSoft)
        outline.rowHeight = 24
        outline.indentationPerLevel = 14
        outline.selectionHighlightStyle = .regular
        outline.style = .plain
        outline.autosaveExpandedItems = false
        outline.focusRingType = .none
        let col = NSTableColumn(identifier: .init("main"))
        col.resizingMask = .autoresizingMask
        outline.addTableColumn(col)
        outline.outlineTableColumn = col
        outline.dataSource = context.coordinator
        outline.delegate = context.coordinator
        outline.registerForDraggedTypes([.string])
        outline.setDraggingSourceOperationMask(.move, forLocal: true)
        context.coordinator.outline = outline

        let scroll = NSScrollView()
        scroll.documentView = outline
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = true
        scroll.backgroundColor = NSColor(Theme.bgSoft)
        context.coordinator.scroll = scroll
        context.coordinator.observe()
        return scroll
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {
        context.coordinator.vm = vm
    }

    @MainActor
    final class Coordinator: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
        var vm: DatabaseViewModel
        let theme: ThemeManager
        weak var outline: NSOutlineView?
        weak var scroll: NSScrollView?
        private var cache: [String: DBItem] = [:]
        private var bag = Set<AnyCancellable>()
        private var applyingExpansion = false

        init(vm: DatabaseViewModel, theme: ThemeManager) { self.vm = vm; self.theme = theme }

        // Native cells capture NSColor(Theme.x) at render, so a theme switch needs an
        // explicit repaint of the backgrounds + a reload to recolor every cell.
        func applyTheme() {
            outline?.backgroundColor = NSColor(Theme.bgSoft)
            scroll?.backgroundColor = NSColor(Theme.bgSoft)
            outline?.reloadData()
        }

        private func item(_ key: String, _ kind: DBItem.Kind) -> DBItem {
            if let c = cache[key] { return c }
            let i = DBItem(key: key, kind: kind); cache[key] = i; return i
        }

        // Reload while preserving the user's expansion, then re-apply persisted state.
        func reload() {
            guard let outline else { return }
            outline.reloadData()
            applyingExpansion = true
            for (repo, _) in vm.grouped where vm.expandedRepos.contains(repo) {
                outline.expandItem(item("b:" + repo, .backend(repo)))
            }
            for (_, dbs) in vm.grouped {
                for db in dbs where vm.expanded.contains(db.id) {
                    outline.expandItem(item("d:" + db.id, .db(db.repo, db.id)))
                }
            }
            applyingExpansion = false
            syncSelection()
        }

        private func syncSelection() {
            guard let outline, let sel = vm.selectedTableID else { return }
            let comps = sel.split(separator: "/", maxSplits: 1).map(String.init)
            guard comps.count == 2 else { return }
            let key = "t:" + sel
            if let it = cache[key] {
                let row = outline.row(forItem: it)
                if row >= 0 { outline.selectRowIndexes([row], byExtendingSelection: false) }
            }
        }

        func observe() {
            let vm = self.vm
            Publishers.MergeMany(
                vm.$databases.map { _ in () }.eraseToAnyPublisher(),
                vm.$tablesByDB.map { _ in () }.eraseToAnyPublisher(),
                vm.$loadingTables.map { _ in () }.eraseToAnyPublisher(),
                vm.$filter.map { _ in () }.eraseToAnyPublisher()
            )
            .receive(on: RunLoop.main)
            .sink { [weak self] in self?.reload() }
            .store(in: &bag)

            vm.$selectedTableID.receive(on: RunLoop.main)
                .sink { [weak self] _ in self?.syncSelection() }
                .store(in: &bag)

            theme.$mode.dropFirst().receive(on: RunLoop.main)
                .sink { [weak self] _ in self?.applyTheme() }
                .store(in: &bag)
        }

        // MARK: data source
        private func dbs(forRepo repo: String) -> [DatabaseViewModel.DB] {
            vm.grouped.first(where: { $0.repo == repo })?.dbs ?? []
        }

        func outlineView(_ ov: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
            guard let it = item as? DBItem else { return vm.grouped.count }
            switch it.kind {
            case .backend(let repo): return dbs(forRepo: repo).count
            case .db(_, let dbID): return vm.filteredTables(dbID).count
            case .table: return 0
            }
        }

        func outlineView(_ ov: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
            guard let it = item as? DBItem else {
                let repo = vm.grouped[index].repo
                return self.item("b:" + repo, .backend(repo))
            }
            switch it.kind {
            case .backend(let repo):
                let db = dbs(forRepo: repo)[index]
                return self.item("d:" + db.id, .db(db.repo, db.id))
            case .db(_, let dbID):
                let t = vm.filteredTables(dbID)[index]
                return self.item("t:" + dbID + "/" + t.id, .table(dbID, t.id))
            case .table:
                return NSNull()
            }
        }

        func outlineView(_ ov: NSOutlineView, isItemExpandable item: Any) -> Bool {
            guard let it = item as? DBItem else { return false }
            switch it.kind { case .table: return false; default: return true }
        }

        // MARK: expansion → sync + lazy load
        func outlineView(_ ov: NSOutlineView, shouldExpandItem item: Any) -> Bool { true }

        func outlineViewItemWillExpand(_ n: Notification) {
            guard let it = n.userInfo?["NSObject"] as? DBItem, !applyingExpansion else { return }
            switch it.kind {
            case .backend(let repo): vm.expandedRepos.insert(repo)
            case .db(let repo, let dbID):
                vm.expanded.insert(dbID)
                if vm.tablesByDB[dbID] == nil, let db = dbs(forRepo: repo).first(where: { $0.id == dbID }) {
                    Task { await self.loadThenReload(db) }
                }
            case .table: break
            }
        }

        private func loadThenReload(_ db: DatabaseViewModel.DB) async {
            await vm.toggleDB(db)   // loads tables if needed (toggle keeps it expanded since already inserted)
            if !vm.expanded.contains(db.id) { vm.expanded.insert(db.id) }
            reload()
            if let outline { outline.expandItem(item("d:" + db.id, .db(db.repo, db.id))) }
        }

        func outlineViewItemDidCollapse(_ n: Notification) {
            guard let it = n.userInfo?["NSObject"] as? DBItem, !applyingExpansion else { return }
            switch it.kind {
            case .backend(let repo): vm.expandedRepos.remove(repo)
            case .db(_, let dbID): vm.expanded.remove(dbID)
            case .table: break
            }
        }

        // MARK: selection
        func outlineViewSelectionDidChange(_ n: Notification) {
            guard let outline, outline.selectedRow >= 0,
                  let it = outline.item(atRow: outline.selectedRow) as? DBItem else { return }
            if case .table(let dbID, let tableID) = it.kind,
               let db = vm.databases.first(where: { $0.id == dbID }),
               let t = (vm.tablesByDB[dbID] ?? []).first(where: { $0.id == tableID }) {
                Task { await vm.openTable(db, t) }
            }
        }

        // MARK: cell
        func outlineView(_ ov: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
            guard let it = item as? DBItem else { return nil }
            let id = NSUserInterfaceItemIdentifier("cell")
            let cell = (ov.makeView(withIdentifier: id, owner: self) as? DBCell) ?? DBCell(id: id)
            switch it.kind {
            case .backend(let repo):
                let engine = dbs(forRepo: repo).first?.engine ?? ""
                cell.configure(icon: engine == "redis" ? "server.rack" : "externaldrive.connected.to.line.below",
                               text: repo, trailing: engine, bold: true, tint: NSColor(Theme.accent))
            case .db(_, let dbID):
                let loading = vm.loadingTables.contains(dbID)
                let name = vm.databases.first(where: { $0.id == dbID })?.display ?? dbID
                cell.configure(icon: "cylinder.split.1x2", text: name, trailing: loading ? "…" : nil,
                               bold: false, tint: NSColor(Theme.accent))
            case .table(let dbID, let tableID):
                let t = (vm.tablesByDB[dbID] ?? []).first(where: { $0.id == tableID })
                let sym = t?.type == "keyspace" ? "key" : (t?.type == "view" ? "eye" : "tablecells")
                cell.configure(icon: sym, text: t?.qualified ?? tableID, trailing: nil,
                               bold: false, tint: NSColor(Theme.dim))
            }
            return cell
        }

        func outlineView(_ ov: NSOutlineView, rowViewForItem item: Any) -> NSTableRowView? {
            DBRowView()
        }

        // MARK: drag reorder (top-level backends only)
        func outlineView(_ ov: NSOutlineView, pasteboardWriterForItem item: Any) -> NSPasteboardWriting? {
            guard let it = item as? DBItem, case .backend(let repo) = it.kind else { return nil }
            let p = NSPasteboardItem(); p.setString(repo, forType: .string); return p
        }

        func outlineView(_ ov: NSOutlineView, validateDrop info: NSDraggingInfo,
                         proposedItem item: Any?, proposedChildIndex index: Int) -> NSDragOperation {
            (item == nil && index >= 0) ? .move : []   // only between top-level rows
        }

        func outlineView(_ ov: NSOutlineView, acceptDrop info: NSDraggingInfo,
                         item: Any?, childIndex index: Int) -> Bool {
            guard let repo = info.draggingPasteboard.string(forType: .string) else { return false }
            vm.moveRepo(repo, toIndex: index)
            reload()
            return true
        }
    }
}

// Themed cell: SF Symbol + label + optional trailing text.
final class DBCell: NSTableCellView {
    private let sym = NSImageView()
    private let label = NSTextField(labelWithString: "")
    private let trail = NSTextField(labelWithString: "")

    init(id: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        identifier = id
        sym.translatesAutoresizingMaskIntoConstraints = false
        label.translatesAutoresizingMaskIntoConstraints = false
        trail.translatesAutoresizingMaskIntoConstraints = false
        label.lineBreakMode = .byTruncatingMiddle
        label.font = .systemFont(ofSize: 12)
        trail.font = .systemFont(ofSize: 9)
        trail.textColor = NSColor(Theme.dim)
        trail.alignment = .right
        addSubview(sym); addSubview(label); addSubview(trail)
        NSLayoutConstraint.activate([
            sym.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2),
            sym.centerYAnchor.constraint(equalTo: centerYAnchor),
            sym.widthAnchor.constraint(equalToConstant: 16),
            label.leadingAnchor.constraint(equalTo: sym.trailingAnchor, constant: 5),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            trail.leadingAnchor.constraint(greaterThanOrEqualTo: label.trailingAnchor, constant: 6),
            trail.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            trail.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    }
    required init?(coder: NSCoder) { fatalError() }

    func configure(icon: String, text: String, trailing: String?, bold: Bool, tint: NSColor) {
        sym.image = NSImage(systemSymbolName: icon, accessibilityDescription: nil)
        sym.contentTintColor = tint
        label.stringValue = text
        label.font = bold ? .boldSystemFont(ofSize: 12) : .systemFont(ofSize: 12)
        label.textColor = NSColor(Theme.fgMuted)
        trail.stringValue = trailing ?? ""
        trail.isHidden = (trailing ?? "").isEmpty
    }
}

// Custom selection fill to match the theme instead of the system blue.
final class DBRowView: NSTableRowView {
    override func drawSelection(in dirtyRect: NSRect) {
        guard isSelected else { return }
        NSColor(Theme.sel).setFill()
        bounds.fill()
    }
}
