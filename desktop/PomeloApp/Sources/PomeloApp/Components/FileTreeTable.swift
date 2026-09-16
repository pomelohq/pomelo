import SwiftUI
import AppKit

// NSTableView-backed file tree: rows are virtualized/reused (smooth at thousands of
// entries), the enclosing scroll view scrolls both axes (Zed-style horizontal scroll),
// and each row hosts the existing SwiftUI row so icons / guides / hover / inline rename
// / context menu are unchanged. Selection is drawn by the SwiftUI row, not the table.
struct FileTreeTable: NSViewRepresentable {
    var rows: [(node: WFileTreeNode, depth: Int)]
    var contentWidth: CGFloat
    var signature: Int   // reload only when structure/selection/edit state changes
    var onTopRow: (Int) -> Void = { _ in }
    var scrollToID: String? = nil   // when this changes, reveal the matching row (minimal scroll)
    let rowView: (WFileTreeNode, Int) -> AnyView

    func makeCoordinator() -> Coordinator { Coordinator(rows: rows, rowView: rowView, onTopRow: onTopRow) }

    func makeNSView(context: Context) -> NSScrollView {
        let table = NSTableView()
        table.headerView = nil
        table.backgroundColor = .clear
        table.selectionHighlightStyle = .none
        table.intercellSpacing = NSSize(width: 0, height: 1)
        table.rowHeight = 22
        table.usesAutomaticRowHeights = false
        if #available(macOS 11.0, *) { table.style = .plain }
        let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("file"))
        col.resizingMask = []
        table.addTableColumn(col)
        table.dataSource = context.coordinator
        table.delegate = context.coordinator
        context.coordinator.table = table

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        context.coordinator.scroll = scroll

        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(context.coordinator,
            selector: #selector(Coordinator.boundsChanged),
            name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let c = context.coordinator
        c.rows = rows
        c.rowView = rowView
        c.onTopRow = onTopRow
        guard let table = c.table, let col = table.tableColumns.first else { return }
        let target = max(scroll.contentSize.width, contentWidth)
        if abs(col.width - target) > 0.5 { col.width = target }
        if c.lastSignature != signature {
            c.lastSignature = signature
            table.reloadData()
        }
        // Reveal the active row. Only advance `lastScrollTarget` once the row actually exists in the current flat
        // list, so a switch into a collapsed folder still scrolls after the expand rebuilds the rows.
        if scrollToID == nil {
            c.lastScrollTarget = nil
        } else if let want = scrollToID, c.lastScrollTarget != want,
                  let idx = c.rows.firstIndex(where: { $0.node.entry?.id == want }) {
            c.lastScrollTarget = want
            DispatchQueue.main.async { [weak c] in c?.reveal(row: idx) }
        }
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        NotificationCenter.default.removeObserver(coordinator)
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        var rows: [(node: WFileTreeNode, depth: Int)]
        var rowView: (WFileTreeNode, Int) -> AnyView
        var onTopRow: (Int) -> Void
        weak var table: NSTableView?
        weak var scroll: NSScrollView?
        var lastSignature = Int.min
        var lastScrollTarget: String?
        private var lastTop = -1

        init(rows: [(node: WFileTreeNode, depth: Int)], rowView: @escaping (WFileTreeNode, Int) -> AnyView, onTopRow: @escaping (Int) -> Void) {
            self.rows = rows; self.rowView = rowView; self.onTopRow = onTopRow
        }

        // Bring the row into view with the MINIMAL scroll: if it's already fully visible, don't move (clicking a
        // visible file shouldn't jerk the tree); otherwise align it to the nearest edge. Centering felt jarring.
        func reveal(row: Int) {
            guard let table, let scroll, row >= 0, row < rows.count else { return }
            let rowRect = table.rect(ofRow: row)
            guard rowRect.height > 0 else { table.scrollRowToVisible(row); return }
            let visible = scroll.contentView.bounds
            if rowRect.minY >= visible.minY && rowRect.maxY <= visible.maxY { return }
            var y = visible.minY
            if rowRect.minY < visible.minY { y = rowRect.minY }                        // above -> align top
            else if rowRect.maxY > visible.maxY { y = rowRect.maxY - visible.height }   // below -> align bottom
            let maxY = max(0, table.bounds.height - visible.height)
            y = min(max(0, y), maxY)
            scroll.contentView.scroll(to: NSPoint(x: visible.minX, y: y))
            scroll.reflectScrolledClipView(scroll.contentView)
        }

        @objc func boundsChanged() {
            guard let table, let scroll else { return }
            let y = scroll.contentView.bounds.minY
            let r = table.row(at: NSPoint(x: 4, y: y + 4))
            let top = r < 0 ? 0 : r
            if top != lastTop { lastTop = top; onTopRow(top) }
        }

        func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard row < rows.count else { return nil }
            let id = NSUserInterfaceItemIdentifier("cell")
            let host: NSHostingView<AnyView>
            if let reused = tableView.makeView(withIdentifier: id, owner: nil) as? NSHostingView<AnyView> {
                host = reused
            } else {
                host = NSHostingView(rootView: AnyView(EmptyView()))
                host.identifier = id
            }
            let (node, depth) = rows[row]
            host.rootView = rowView(node, depth)
            return host
        }

        // The SwiftUI row owns interaction; the table never takes selection.
        func selectionShouldChange(in tableView: NSTableView) -> Bool { false }
        func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
    }
}
