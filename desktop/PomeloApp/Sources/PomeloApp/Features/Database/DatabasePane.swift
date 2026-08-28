import SwiftUI
import AppKit

struct DatabasePane: View {
    let workspace: Workspace
    @StateObject private var vm: DatabaseViewModel
    @EnvironmentObject var theme: ThemeManager
    @AppStorage("db.treeWidth") private var treeWidth = 250.0
    @AppStorage("db.editorHeight") private var editorHeight = 150.0

    init(workspace: Workspace) {
        self.workspace = workspace
        _vm = StateObject(wrappedValue: DatabaseViewModel(branch: workspace.branch))
    }

    var body: some View {
        HStack(spacing: 0) {
            treeExplorer.frame(width: treeWidth)
            SplitHandle(axis: .horizontal, value: $treeWidth, min: 180, max: 520)
            queryArea.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Theme.bg)
        .task { await vm.loadDatabases() }
    }


    private var treeExplorer: some View {
        VStack(spacing: 0) {
            HStack(spacing: 5) {
                Image(systemName: "magnifyingglass").font(.system(size: 10)).foregroundStyle(Theme.dim)
                TextField("Filter tables", text: $vm.filter).textFieldStyle(.plain).font(.system(size: 12))
                if vm.loadingDBs { Spinner(size: 12) }
            }
            .padding(.horizontal, 8).padding(.vertical, 6)
            Divider().overlay(Theme.borderSoft)
            if let e = vm.error { errorBanner(e) }
            if vm.databases.isEmpty && !vm.loadingDBs {
                EmptyStateView(icon: "cylinder.split.1x2", title: "No databases")
            } else {
                DBTree(vm: vm)
            }
        }
        .background(Theme.bgSoft)
    }

    private func errorBanner(_ e: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 10)).foregroundStyle(Theme.danger)
            Text(e).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.danger).lineLimit(2)
            Spacer()
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .background(Theme.dangerSoft)
    }


    private var queryArea: some View {
        VStack(spacing: 0) {
            consoleTabs
            Divider().overlay(Theme.borderSoft)
            if vm.activeConsole?.isTable == true {
                tableToolbar
                Divider().overlay(Theme.borderSoft)
                resultsArea
            } else {
                consoleToolbar
                Divider().overlay(Theme.borderSoft)
                sqlEditor.frame(height: editorHeight)
                SplitHandle(axis: .vertical, value: $editorHeight, min: 60, max: 480)
                resultsArea
            }
        }
    }

    private var tableToolbar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "tablecells").font(.system(size: 11)).foregroundStyle(Theme.accent)
                Text(vm.activeConsole?.title ?? "").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.fg)
                if let repo = vm.selected?.repo { Text(repo).font(.system(size: 10)).foregroundStyle(Theme.dim) }
                Spacer()
                pageSizeMenu
                if vm.running { Spinner(size: 12) }
                Button { Task { await vm.run() } } label: {
                    Image(systemName: "arrow.clockwise").font(.system(size: 11)).foregroundStyle(Theme.fgMuted)
                        .frame(width: 24, height: 20).contentShape(Rectangle())
                }
                .buttonStyle(.plain).disabled(vm.running)
                .keyboardShortcut("r", modifiers: .command)
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            HStack(spacing: 6) {
                clauseField("WHERE", get: { vm.activeConsole?.whereClause ?? "" }, set: { vm.setWhere($0) })
                clauseField("ORDER BY", get: { vm.activeConsole?.orderBy ?? "" }, set: { vm.setOrderBy($0) })
            }
            .padding(.horizontal, 8).padding(.bottom, 5)
        }
        .background(Theme.bgSoft)
    }

    private func clauseField(_ label: String, get: @escaping () -> String, set: @escaping (String) -> Void) -> some View {
        HStack(spacing: 5) {
            Text(label).font(Theme.mono(9.5, .semibold)).foregroundStyle(Theme.dim)
            TextField("", text: Binding(get: get, set: set))
                .textFieldStyle(.plain).font(Theme.mono(11)).foregroundStyle(Theme.fg)
                .onSubmit { Task { await vm.run() } }
        }
        .padding(.horizontal, 8).padding(.vertical, 4)
        .background(Theme.bg, in: RoundedRectangle(cornerRadius: 5))
        .overlay(RoundedRectangle(cornerRadius: 5).stroke(Theme.borderSoft, lineWidth: 1))
        .frame(maxWidth: .infinity)
    }

    private var pageSizeMenu: some View {
        Menu {
            ForEach([10, 100, 250, 500, 1000, 5000], id: \.self) { n in
                Button("\(n) rows") { vm.setLimit(n) }
            }
        } label: {
            HStack(spacing: 3) {
                Text("\(vm.activeConsole?.limit ?? 500)").font(.system(size: 11))
                Image(systemName: "chevron.down").font(.system(size: 7))
            }
            .foregroundStyle(Theme.fgMuted).padding(.horizontal, 7).padding(.vertical, 3)
            .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 5))
        }
        .menuStyle(.borderlessButton).fixedSize()
        .help("Rows per query")
    }

    private var consoleTabs: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 2) {
                ForEach(vm.consoles) { c in
                    Button { vm.activate(c.id) } label: {
                        HStack(spacing: 6) {
                            Image(systemName: c.isTable ? "tablecells" : "doc.text").font(.system(size: 9))
                            Text(c.title).font(.system(size: 11)).lineLimit(1)
                            if vm.consoles.count > 1 {
                                Button { vm.closeConsole(c.id) } label: { Image(systemName: "xmark").font(.system(size: 8)) }
                                    .buttonStyle(.plain).foregroundStyle(Theme.dim)
                            }
                        }
                        .foregroundStyle(vm.activeID == c.id ? Theme.fg : Theme.fgMuted)
                        .padding(.horizontal, 9).padding(.vertical, 5)
                        .background(vm.activeID == c.id ? Theme.panel3 : .clear, in: RoundedRectangle(cornerRadius: 6))
                    }
                    .buttonStyle(.plain)
                    .contextMenu {
                        Button("Close") { vm.closeConsole(c.id) }
                        Button("Close Others") { vm.closeOtherConsoles(c.id) }
                        Button("Close to the Right") { vm.closeConsolesToRight(c.id) }
                        Divider()
                        Button("Close All") { vm.closeAllConsoles() }
                    }
                }
                Button { vm.newConsole() } label: { Image(systemName: "plus").font(.system(size: 11)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.fgMuted).padding(.horizontal, 4)
                Spacer()
            }
            .padding(.horizontal, 8).padding(.vertical, 4)
        }
        .background(Theme.bgSoft)
    }

    private var consoleToolbar: some View {
        HStack(spacing: 8) {
            Menu {
                ForEach(vm.grouped, id: \.repo) { group in
                    Section("\(group.repo) · \(group.dbs.first?.engine ?? "")") {
                        ForEach(group.dbs) { db in
                            Button { vm.setConsoleDB(db.name) } label: {
                                Label(db.display, systemImage: db.name == vm.selectedDB ? "checkmark" : "")
                            }
                        }
                    }
                }
            } label: {
                HStack(spacing: 5) {
                    Image(systemName: "cylinder.split.1x2").font(.system(size: 10)).foregroundStyle(Theme.accent)
                    if let sel = vm.selected {
                        Text(sel.repo).font(.system(size: 11)).foregroundStyle(Theme.dim)
                        Text("·").foregroundStyle(Theme.dim)
                        Text(sel.display).font(.system(size: 11, weight: .medium))
                    } else {
                        Text("Select database").font(.system(size: 11, weight: .medium))
                    }
                    Image(systemName: "chevron.down").font(.system(size: 8))
                }
                .foregroundStyle(Theme.fg)
                .padding(.horizontal, 8).padding(.vertical, 3)
                .background(Theme.panel3, in: RoundedRectangle(cornerRadius: 6))
            }
            .menuStyle(.borderlessButton).fixedSize()
            if let db = vm.selectedDB {
                let n = vm.tablesByDB[db]?.count
                Text(n == nil ? "loading schema…" : "\(n!) tables")
                    .font(.system(size: 10)).foregroundStyle(n == nil ? Theme.warn : Theme.dim)
            }
            Spacer()
            pageSizeMenu
            if vm.running { Spinner(size: 12) }
            Button { Task { await vm.run() } } label: {
                Label("Run", systemImage: "play.fill").font(.system(size: 11, weight: .medium))
                    .padding(.horizontal, 10).padding(.vertical, 4)
            }
            .buttonStyle(.plain).foregroundStyle(.white)
            .background(Theme.accent, in: RoundedRectangle(cornerRadius: 6))
            .keyboardShortcut(.return, modifiers: .command)
            .disabled(vm.running || vm.selectedDB == nil)
        }
        .padding(.horizontal, 8).padding(.vertical, 5)
        .background(Theme.bgSoft)
    }

    private var sqlEditor: some View {
        SQLEditor(text: Binding(get: { vm.activeSQLGet() }, set: { vm.activeSQLSet($0) }),
                  mode: theme.mode,
                  keywords: DatabaseViewModel.sqlKeywords,
                  tables: vm.completionTables,
                  columns: vm.completionColumns,
                  onRun: { Task { await vm.run() } })
            .background(Theme.bg)
            .id(vm.activeID)
    }

    private var resultsArea: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                VStack(spacing: 0) { resultGrid }.frame(maxWidth: .infinity, maxHeight: .infinity)
                if vm.showRecord {
                    Divider().overlay(Theme.borderSoft)
                    recordPanel.frame(width: 300)
                }
            }
            resultStatus
        }
        .frame(maxHeight: .infinity)
    }

    @ViewBuilder private var resultGrid: some View {
        if vm.grid.columns.isEmpty && vm.grid.rows.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: "tablecells").font(.system(size: 30)).foregroundStyle(Theme.dim)
                Text(vm.grid.rowsAffected > 0 ? "\(vm.grid.rowsAffected) rows affected"
                     : (vm.isRedis ? "Pick a keyspace or run a Redis command (e.g. GET key)" : "Pick a table or run a query"))
                    .font(.system(size: 12)).foregroundStyle(Theme.dim)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            DataGrid(columns: vm.grid.columns, rows: vm.grid.rows, isDark: theme.mode.isDark,
                     onSelectRow: { vm.selectedRow = $0 })
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var recordPanel: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Record").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.fg)
                Spacer()
                Button { vm.showRecord = false } label: { Image(systemName: "xmark").font(.system(size: 9)) }
                    .buttonStyle(.plain).foregroundStyle(Theme.dim)
            }
            .padding(.horizontal, 10).padding(.vertical, 6)
            Divider().overlay(Theme.borderSoft)
            if let r = vm.selectedRow, r < vm.grid.rows.count {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(vm.grid.columns.enumerated()), id: \.offset) { ci, col in
                            let val = ci < vm.grid.rows[r].count ? vm.grid.rows[r][ci] : ""
                            let pretty = Self.prettyJSON(val)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(col).font(Theme.mono(9.5, .semibold)).foregroundStyle(Theme.dim)
                                HStack(alignment: .top, spacing: 6) {
                                    Text(val.isEmpty ? "—" : (pretty ?? val)).font(Theme.mono(11.5))
                                        .foregroundStyle(val == "NULL" ? Theme.dim : Theme.fg)
                                        .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                                    CopyMini(text: pretty ?? val)
                                }
                            }
                            .padding(.horizontal, 10).padding(.vertical, 5)
                            Divider().overlay(Theme.borderSoft.opacity(0.35))
                        }
                    }
                }
            } else {
                Text("Select a row").font(.system(size: 11)).foregroundStyle(Theme.dim)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Theme.bgSoft)
    }

    private var resultStatus: some View {
        HStack(spacing: 8) {
            if vm.activeConsole?.isTable == true {
                pageControls
            } else if !vm.grid.rows.isEmpty {
                Text("\(vm.grid.rows.count) row\(vm.grid.rows.count == 1 ? "" : "s")")
                    .font(.system(size: 10)).foregroundStyle(Theme.dim)
            }
            if vm.grid.truncated && vm.activeConsole?.isTable != true {
                Text("· capped at \(vm.activeConsole?.limit ?? 500)").font(.system(size: 10)).foregroundStyle(Theme.warn)
            }
            if let m = vm.exportMsg { Text("· \(m)").font(.system(size: 10)).foregroundStyle(Theme.ok).lineLimit(1) }
            Spacer()
            if vm.exporting { Spinner(size: 12) }
            Button { vm.showRecord.toggle() } label: {
                Image(systemName: "sidebar.right").font(.system(size: 10))
                    .foregroundStyle(vm.showRecord ? Theme.accent : Theme.fgMuted)
            }.buttonStyle(.plain).help("Row detail")
            Button { exportCSV() } label: {
                Label("Export CSV", systemImage: "square.and.arrow.up").font(.system(size: 10))
                    .foregroundStyle(Theme.fgMuted)
            }.buttonStyle(.plain).disabled(vm.exporting || vm.grid.columns.isEmpty).help("Export full result (streamed)")
        }
        .padding(.horizontal, 10).padding(.vertical, 3)
        .background(Theme.bgSoft)
        .overlay(Rectangle().fill(Theme.borderSoft).frame(height: 1), alignment: .top)
    }

    private var pageControls: some View {
        HStack(spacing: 6) {
            pageBtn("chevron.left.to.line", enabled: vm.canPrevPage) { vm.firstPage() }
            pageBtn("chevron.left", enabled: vm.canPrevPage) { vm.prevPage() }
            Text(vm.pageRange).font(Theme.mono(10)).foregroundStyle(Theme.dim).frame(minWidth: 60)
            pageBtn("chevron.right", enabled: vm.canNextPage) { vm.nextPage() }
        }
    }
    private func pageBtn(_ icon: String, enabled: Bool, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon).font(.system(size: 10))
                .foregroundStyle(enabled ? Theme.fgMuted : Theme.dim.opacity(0.4))
                .frame(width: 18, height: 16).contentShape(Rectangle())
        }.buttonStyle(.plain).disabled(!enabled)
    }

    static func prettyJSON(_ s: String) -> String? {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines)
        guard t.hasPrefix("{") || t.hasPrefix("["), let data = t.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data),
              let out = try? JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted, .sortedKeys]),
              let str = String(data: out, encoding: .utf8) else { return nil }
        return str
    }

    private func exportCSV() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = (vm.activeConsole?.table.isEmpty == false ? vm.activeConsole!.table : "query") + ".csv"
        panel.allowedContentTypes = [.commaSeparatedText]
        panel.canCreateDirectories = true
        if panel.runModal() == .OK, var path = panel.url?.path {
            if !path.lowercased().hasSuffix(".csv") { path += ".csv" }   // ensure extension
            Task { await vm.exportCSV(to: path) }
        }
    }
}

struct SplitHandle: View {
    enum Axis { case horizontal, vertical }
    let axis: Axis
    @Binding var value: Double
    let min: Double
    let max: Double
    @State private var start: Double?
    @State private var active = false

    var body: some View {
        let line = active ? Theme.accent : Theme.borderSoft
        ZStack {
            Theme.bgSoft
            Rectangle().fill(line)
                .frame(width: axis == .horizontal ? 1 : nil, height: axis == .vertical ? 1 : nil)
        }
        .frame(width: axis == .horizontal ? 6 : nil, height: axis == .vertical ? 6 : nil)
        .contentShape(Rectangle())
        .onHover { $0 ? (axis == .horizontal ? NSCursor.resizeLeftRight : NSCursor.resizeUpDown).set()
                      : (active ? () : NSCursor.arrow.set()) }
        .gesture(
            DragGesture(minimumDistance: 0, coordinateSpace: .global)
                .onChanged { g in
                    if start == nil { start = value; active = true }
                    let delta = axis == .horizontal ? g.translation.width : g.translation.height
                    value = Swift.min(max, Swift.max(min, (start ?? value) + delta))
                }
                .onEnded { _ in start = nil; active = false; NSCursor.arrow.set() }
        )
    }
}
