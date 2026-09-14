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
    let rowView: (WFileTreeNode, Int) -> AnyView

    func makeCoordinator() -> Coordinator { Coordinator(rows: rows, rowView: rowView) }

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
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let c = context.coordinator
        c.rows = rows
        c.rowView = rowView
        guard let table = c.table, let col = table.tableColumns.first else { return }
        let target = max(scroll.contentSize.width, contentWidth)
        if abs(col.width - target) > 0.5 { col.width = target }
        if c.lastSignature != signature {
            c.lastSignature = signature
            table.reloadData()
        }
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        var rows: [(node: WFileTreeNode, depth: Int)]
        var rowView: (WFileTreeNode, Int) -> AnyView
        weak var table: NSTableView?
        weak var scroll: NSScrollView?
        var lastSignature = Int.min

        init(rows: [(node: WFileTreeNode, depth: Int)], rowView: @escaping (WFileTreeNode, Int) -> AnyView) {
            self.rows = rows; self.rowView = rowView
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
