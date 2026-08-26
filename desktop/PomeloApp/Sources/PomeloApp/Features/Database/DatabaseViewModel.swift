import Foundation

@MainActor
final class DatabaseViewModel: ObservableObject {
    struct DB: Decodable, Identifiable, Equatable {
        var name = "", engine = "", repo = "", label = ""
        var id: String { name }
        var display: String { label.isEmpty ? name : label }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
            engine = try c.decodeIfPresent(String.self, forKey: .engine) ?? ""
            repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
            label = try c.decodeIfPresent(String.self, forKey: .label) ?? ""
        }
        enum K: String, CodingKey { case name, engine, repo, label }
    }
    struct Table: Decodable, Identifiable, Equatable {
        var schema = ""; var name = ""; var type = ""
        var id: String { schema + "." + name }
        var qualified: String { (schema.isEmpty || schema == "public") ? name : schema + "." + name }
    }
    struct Grid: Equatable {
        var columns: [String] = []
        var rows: [[String]] = []
        var truncated = false
        var rowsAffected: Int64 = 0
    }
    struct Console: Identifiable, Codable, Equatable {
        var id: String
        var title: String
        var dbID: String   // target database name ("" = none chosen yet)
        var sql: String
        var kind: String = "query"   // "query" | "table"
        var schema: String = ""
        var table: String = ""
        var whereClause: String = ""   // table tab: WHERE …
        var orderBy: String = ""       // table tab: ORDER BY …
        var limit: Int = 500
        var isTable: Bool { kind == "table" }

        init(id: String, title: String, dbID: String, sql: String,
             kind: String = "query", schema: String = "", table: String = "",
             whereClause: String = "", orderBy: String = "", limit: Int = 500) {
            self.id = id; self.title = title; self.dbID = dbID; self.sql = sql
            self.kind = kind; self.schema = schema; self.table = table
            self.whereClause = whereClause; self.orderBy = orderBy; self.limit = limit
        }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            id = try c.decode(String.self, forKey: .id)
            title = (try? c.decode(String.self, forKey: .title)) ?? "query"
            dbID = (try? c.decode(String.self, forKey: .dbID)) ?? ""
            sql = (try? c.decode(String.self, forKey: .sql)) ?? ""
            kind = (try? c.decode(String.self, forKey: .kind)) ?? "query"
            schema = (try? c.decode(String.self, forKey: .schema)) ?? ""
            table = (try? c.decode(String.self, forKey: .table)) ?? ""
            whereClause = (try? c.decode(String.self, forKey: .whereClause)) ?? ""
            orderBy = (try? c.decode(String.self, forKey: .orderBy)) ?? ""
            limit = (try? c.decode(Int.self, forKey: .limit)) ?? 500
        }
        enum K: String, CodingKey { case id, title, dbID, sql, kind, schema, table, whereClause, orderBy, limit }
    }

    private struct DBList: Decodable {
        var ok = false, error = ""; var databases: [DB] = []
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? false
            error = try c.decodeIfPresent(String.self, forKey: .error) ?? ""
            databases = try c.decodeIfPresent([DB].self, forKey: .databases) ?? []
        }
        enum K: String, CodingKey { case ok, error, databases }
    }
    private struct TableList: Decodable {
        var ok = false, error = ""; var tables: [Table] = []
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? false
            error = try c.decodeIfPresent(String.self, forKey: .error) ?? ""
            tables = try c.decodeIfPresent([Table].self, forKey: .tables) ?? []
        }
        enum K: String, CodingKey { case ok, error, tables }
    }
    private struct QueryResult: Decodable {
        var ok = false, error = ""; var columns: [String] = []; var rows: [[String]] = []
        var truncated = false; var rows_affected: Int64 = 0
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? false
            error = try c.decodeIfPresent(String.self, forKey: .error) ?? ""
            columns = try c.decodeIfPresent([String].self, forKey: .columns) ?? []
            rows = try c.decodeIfPresent([[String]].self, forKey: .rows) ?? []
            truncated = try c.decodeIfPresent(Bool.self, forKey: .truncated) ?? false
            rows_affected = try c.decodeIfPresent(Int64.self, forKey: .rows_affected) ?? 0
        }
        enum K: String, CodingKey { case ok, error, columns, rows, truncated, rows_affected }
    }

    let branch: String
    private let api: DBAPI
    init(branch: String, api: DBAPI = PomCore.shared) {
        self.branch = branch; self.api = api
        repoOrder = (UserDefaults.standard.array(forKey: "db.repoOrder") as? [String]) ?? []
    }

    @Published var repoOrder: [String] = [] {
        didSet { UserDefaults.standard.set(repoOrder, forKey: "db.repoOrder") }
    }

    @Published private(set) var databases: [DB] = []
    var selectedDB: String? { activeConsole.flatMap { $0.dbID.isEmpty ? nil : $0.dbID } }
    var selected: DB? { databases.first { $0.name == selectedDB } }
    var engine: String { selected?.engine ?? "postgres" }
    var isRedis: Bool { engine == "redis" }

    var grouped: [(repo: String, dbs: [DB])] {
        var order: [String] = []
        var byRepo: [String: [DB]] = [:]
        for db in databases {
            if byRepo[db.repo] == nil { order.append(db.repo) }
            byRepo[db.repo, default: []].append(db)
        }
        if !repoOrder.isEmpty {
            let saved = repoOrder.filter { byRepo[$0] != nil }
            order = saved + order.filter { !repoOrder.contains($0) }
        }
        return order.map { ($0, byRepo[$0] ?? []) }
    }

    func moveRepo(_ dragged: String, before target: String) {
        guard dragged != target else { return }
        var order = grouped.map { $0.repo }
        guard let from = order.firstIndex(of: dragged) else { return }
        order.remove(at: from)
        guard let to = order.firstIndex(of: target) else { return }
        order.insert(dragged, at: to)
        repoOrder = order
    }
    func moveRepo(_ dragged: String, toIndex t: Int) {
        var order = grouped.map { $0.repo }
        guard let from = order.firstIndex(of: dragged) else { return }
        order.remove(at: from)
        order.insert(dragged, at: min(max(t, 0), order.count))
        repoOrder = order
    }
    @Published var expanded: Set<String> = [] {
        didSet { UserDefaults.standard.set(Array(expanded), forKey: "db.expand." + branch) }
    }
    @Published var expandedRepos: Set<String> = [] {
        didSet { UserDefaults.standard.set(Array(expandedRepos), forKey: "db.expandRepo." + branch) }
    }
    @Published private(set) var tablesByDB: [String: [Table]] = [:]
    @Published private(set) var loadingTables: Set<String> = []
    @Published var selectedTableID: String?              // "<dbid>/<tableid>"
    @Published private(set) var loadingDBs = false
    @Published private(set) var running = false
    @Published var error: String?
    @Published var filter = ""

    @Published var consoles: [Console] = []
    @Published var activeID: String?
    @Published private(set) var results: [String: Grid] = [:]
    @Published private(set) var columnsByDB: [String: [String]] = [:]

    static let sqlKeywords = ["SELECT","FROM","WHERE","INSERT","INTO","VALUES","UPDATE","SET","DELETE",
        "CREATE","TABLE","ALTER","DROP","JOIN","LEFT","RIGHT","INNER","OUTER","ON","AND","OR","NOT",
        "NULL","IS","IN","LIKE","LIMIT","OFFSET","ORDER","BY","GROUP","HAVING","DISTINCT","AS","COUNT",
        "SUM","AVG","MIN","MAX","CASE","WHEN","THEN","ELSE","END","UNION","ALL","ASC","DESC","RETURNING","WITH"]
    var completionTables: [String] {
        let db = selectedDB ?? ""
        if let ts = tablesByDB[db], !ts.isEmpty { return ts.map(\.name).sorted() }
        return Array(Set(tablesByDB.values.flatMap { $0 }.map(\.name))).sorted()
    }
    var completionColumns: [String] {
        let db = selectedDB ?? ""
        if let cs = columnsByDB[db], !cs.isEmpty { return cs.sorted() }
        return Array(Set(columnsByDB.values.flatMap { $0 })).sorted()
    }

    var activeConsole: Console? { consoles.first { $0.id == activeID } }
    var grid: Grid { activeID.flatMap { results[$0] } ?? Grid() }

    private var sqlBuffer: [String: String] = [:]
    func activeSQLGet() -> String { activeID.flatMap { sqlBuffer[$0] } ?? "" }
    func activeSQLSet(_ s: String) {
        guard let id = activeID else { return }
        sqlBuffer[id] = s
        scheduleSave()
    }

    func filteredTables(_ dbID: String) -> [Table] {
        let all = tablesByDB[dbID] ?? []
        guard !filter.isEmpty else { return all }
        let q = filter.lowercased()
        return all.filter { $0.name.lowercased().contains(q) || $0.schema.lowercased().contains(q) }
    }

    func loadDatabases() async {
        await loadConsoles()
        guard databases.isEmpty else { return }
        loadingDBs = true; error = nil
        let d = await api.call { [branch] in $0.dbListData(branch: branch) }
        loadingDBs = false
        guard let r = PomJSON.decode(DBList.self, from: d) else { error = "decode failed"; return }
        if !r.ok { error = r.error.isEmpty ? "failed to list databases" : r.error; return }
        databases = r.databases
        if let s = UserDefaults.standard.array(forKey: "db.expandRepo." + branch) as? [String] {
            expandedRepos = Set(s)
        } else {
            expandedRepos = Set(grouped.map { $0.repo })
        }
        if let s = UserDefaults.standard.array(forKey: "db.expand." + branch) as? [String] {
            expanded = Set(s)
        }
        if let id = activeID, let i = consoles.firstIndex(where: { $0.id == id }), consoles[i].dbID.isEmpty {
            consoles[i].dbID = databases.first?.name ?? ""
        }
        for db in databases where expanded.contains(db.id) && tablesByDB[db.id] == nil {
            await loadTables(db)
        }
        if expanded.isEmpty, let first = databases.first { await toggleDB(first) }
        if let db = selectedDB { await ensureSchema(db) }
    }

    func toggleRepo(_ repo: String) {
        if expandedRepos.contains(repo) { expandedRepos.remove(repo) } else { expandedRepos.insert(repo) }
    }

    func toggleDB(_ db: DB) async {
        if expanded.contains(db.id) { expanded.remove(db.id); return }
        expanded.insert(db.id)
        if tablesByDB[db.id] == nil { await loadTables(db) }
    }

    private func loadTables(_ db: DB) async {
        loadingTables.insert(db.id); error = nil
        let d = await api.call { [branch] in $0.dbTablesData(branch: branch, db: db.name) }
        loadingTables.remove(db.id)
        guard let r = PomJSON.decode(TableList.self, from: d) else { error = "decode failed"; return }
        if !r.ok { error = r.error.isEmpty ? "failed to list tables" : r.error; return }
        tablesByDB[db.id] = r.tables
    }

    func openTable(_ db: DB, _ t: Table) async {
        selectedTableID = db.id + "/" + t.id
        if let existing = consoles.first(where: {
            $0.isTable && $0.dbID == db.name && $0.schema == t.schema && $0.table == t.name
        }) {
            activeID = existing.id
        } else {
            let c = Console(id: UUID().uuidString, title: t.qualified, dbID: db.name, sql: "",
                            kind: "table", schema: t.schema, table: t.name)
            consoles.append(c); activeID = c.id; save()
        }
        await ensureSchema(db.name)
        await run()
    }

    private func queryFor(_ c: Console) -> String {
        guard c.isTable else { return sqlBuffer[c.id] ?? c.sql }
        let engine = databases.first(where: { $0.name == c.dbID })?.engine ?? "postgres"
        if engine == "redis" { return c.table }
        let qualified = (c.schema.isEmpty || c.schema == "public") ? c.table : "\(c.schema).\(c.table)"
        var q = "SELECT * FROM \(qualified)"
        let w = c.whereClause.trimmingCharacters(in: .whitespaces)
        if !w.isEmpty { q += " WHERE \(w)" }
        let o = c.orderBy.trimmingCharacters(in: .whitespaces)
        if !o.isEmpty { q += " ORDER BY \(o)" }
        q += " LIMIT \(c.limit)"
        let off = pageOffset[c.id] ?? 0
        if off > 0 { q += " OFFSET \(off)" }
        return q
    }

    @Published var pageOffset: [String: Int] = [:]
    @Published var totalRows: [String: Int] = [:]
    var canPrevPage: Bool { (pageOffset[activeID ?? ""] ?? 0) > 0 }
    var canNextPage: Bool {
        guard activeConsole?.isTable == true, let id = activeID else { return false }
        if let t = totalRows[id] { return (pageOffset[id] ?? 0) + grid.rows.count < t }
        return grid.rows.count >= (activeConsole?.limit ?? 500)
    }
    var pageRange: String {
        let off = pageOffset[activeID ?? ""] ?? 0, n = grid.rows.count
        let base = n == 0 ? "0" : "\(off + 1)–\(off + n)"
        if let id = activeID, let t = totalRows[id] { return "\(base) of \(t)" }
        return base
    }
    private func loadTotal(_ c: Console, id: String) async {
        let engine = databases.first(where: { $0.name == c.dbID })?.engine ?? "postgres"
        guard engine != "redis" else { return }
        let qualified = (c.schema.isEmpty || c.schema == "public") ? c.table : "\(c.schema).\(c.table)"
        var q = "SELECT count(*) FROM \(qualified)"
        let w = c.whereClause.trimmingCharacters(in: .whitespaces); if !w.isEmpty { q += " WHERE \(w)" }
        let db = c.dbID
        let d = await api.call { [branch] in $0.dbQueryData(branch: branch, db: db, sql: q, limit: 1) }
        guard let r = PomJSON.decode(QueryResult.self, from: d), r.ok,
              let first = r.rows.first?.first, let n = Int(first) else { return }
        totalRows[id] = n
    }

    func nextPage() { guard let id = activeID, let c = activeConsole else { return }; pageOffset[id] = (pageOffset[id] ?? 0) + c.limit; Task { await run() } }
    func prevPage() { guard let id = activeID, let c = activeConsole else { return }; pageOffset[id] = Swift.max(0, (pageOffset[id] ?? 0) - c.limit); Task { await run() } }
    func firstPage() { guard let id = activeID else { return }; pageOffset[id] = 0; Task { await run() } }

    func run() async {
        guard let id = activeID, let c = activeConsole, !c.dbID.isEmpty else { return }
        let q = queryFor(c)
        guard !q.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        running = true; error = nil
        let db = c.dbID, lim = c.limit
        let d = await api.call { [branch] in $0.dbQueryData(branch: branch, db: db, sql: q, limit: lim) }
        running = false
        guard let r = PomJSON.decode(QueryResult.self, from: d) else { error = "decode failed"; return }
        if !r.ok { error = r.error.isEmpty ? "query failed" : r.error; return }
        results[id] = Grid(columns: r.columns, rows: r.rows, truncated: r.truncated, rowsAffected: r.rows_affected)
        if c.isTable { await loadTotal(c, id: id) }
    }

    @Published var showRecord = false
    @Published var selectedRow: Int?
    @Published var exporting = false
    @Published var exportMsg: String?

    private func exportSQL(_ c: Console) -> String {
        guard c.isTable else { return sqlBuffer[c.id] ?? c.sql }
        let engine = databases.first(where: { $0.name == c.dbID })?.engine ?? "postgres"
        if engine == "redis" { return "" }
        let qualified = (c.schema.isEmpty || c.schema == "public") ? c.table : "\(c.schema).\(c.table)"
        var q = "SELECT * FROM \(qualified)"
        let w = c.whereClause.trimmingCharacters(in: .whitespaces); if !w.isEmpty { q += " WHERE \(w)" }
        let o = c.orderBy.trimmingCharacters(in: .whitespaces); if !o.isEmpty { q += " ORDER BY \(o)" }
        return q
    }

    func exportCSV(to path: String) async {
        guard let c = activeConsole, !c.dbID.isEmpty else { return }
        let sql = exportSQL(c)
        guard !sql.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { exportMsg = "Nothing to export"; return }
        exporting = true; exportMsg = nil
        let db = c.dbID
        let d = await api.call { [branch] in $0.dbExportCSV(branch: branch, db: db, sql: sql, path: path) }
        exporting = false
        let obj = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any]
        if obj?["ok"] as? Bool == true {
            let rows = (obj?["rows"] as? Int) ?? 0
            exportMsg = "Exported \(rows) rows → \(path)"
        } else {
            exportMsg = (obj?["error"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "Export failed"
        }
    }

    private func mutateActive(_ f: (inout Console) -> Void) {
        guard let id = activeID, let i = consoles.firstIndex(where: { $0.id == id }) else { return }
        f(&consoles[i]); save()
    }
    func setWhere(_ s: String) { mutateActive { $0.whereClause = s }; if let id = activeID { pageOffset[id] = 0 } }
    func setOrderBy(_ s: String) { mutateActive { $0.orderBy = s }; if let id = activeID { pageOffset[id] = 0 } }
    func setLimit(_ n: Int) { mutateActive { $0.limit = n }; if let id = activeID { pageOffset[id] = 0 }; Task { await run() } }


    func newConsole(dbID: String? = nil) {
        let n = consoles.count + 1
        let db = dbID ?? databases.first?.name ?? ""
        let c = Console(id: UUID().uuidString, title: "query \(n)", dbID: db, sql: "")
        sqlBuffer[c.id] = ""
        consoles.append(c); activeID = c.id; save()
        if !db.isEmpty { Task { await ensureSchema(db) } }
    }

    func activate(_ id: String) {
        activeID = id
        if let db = consoles.first(where: { $0.id == id })?.dbID, !db.isEmpty {
            Task { await ensureSchema(db) }
        }
    }

    func closeConsole(_ id: String) {
        forget([id])
        consoles.removeAll { $0.id == id }
        if activeID == id { activeID = consoles.last?.id }
        if consoles.isEmpty { newConsole() }
        save()
    }

    func closeOtherConsoles(_ id: String) {
        forget(consoles.map(\.id).filter { $0 != id })
        consoles.removeAll { $0.id != id }
        activeID = id; save()
    }

    func closeConsolesToRight(_ id: String) {
        guard let i = consoles.firstIndex(where: { $0.id == id }) else { return }
        forget(consoles[(i + 1)...].map(\.id))
        consoles = Array(consoles[...i])
        if let a = activeID, !consoles.contains(where: { $0.id == a }) { activeID = id }
        save()
    }

    func closeAllConsoles() {
        forget(consoles.map(\.id))
        consoles.removeAll(); activeID = nil; newConsole()
    }

    private func forget(_ ids: [String]) {
        for id in ids { results[id] = nil; sqlBuffer[id] = nil; pageOffset[id] = nil; totalRows[id] = nil }
    }

    func setConsoleDB(_ dbID: String) {
        guard let id = activeID, let i = consoles.firstIndex(where: { $0.id == id }) else { return }
        consoles[i].dbID = dbID; save()
        Task { await ensureSchema(dbID) }
    }

    func ensureSchema(_ dbName: String) async {
        if tablesByDB[dbName] == nil,
           let db = databases.first(where: { $0.name == dbName }) {
            await loadTables(db)
        }
        if columnsByDB[dbName] == nil {
            let d = await api.call { [branch] in $0.dbColumnsData(branch: branch, db: dbName) }
            struct Col: Decodable { var name = "" }
            struct R: Decodable { var columns: [Col] = [] }
            let cols = (PomJSON.decode(R.self, from: d)?.columns) ?? []
            columnsByDB[dbName] = Array(Set(cols.map { $0.name })).sorted()
        }
    }

    private struct ConsolesResponse: Decodable {
        var consoles: [Console] = []
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            consoles = (try? c.decode([Console].self, forKey: .consoles)) ?? []
        }
        enum K: String, CodingKey { case consoles }
    }

    private func loadConsoles() async {
        guard consoles.isEmpty else { return }
        let d = await api.call { $0.dbConsolesLoadData() }
        let loaded = (PomJSON.decode(ConsolesResponse.self, from: d)?.consoles) ?? []
        consoles = loaded
        loaded.forEach { sqlBuffer[$0.id] = $0.sql }
        activeID = loaded.first?.id
        if consoles.isEmpty { newConsole(dbID: "") }
    }

    private var saveTask: Task<Void, Never>?
    private func scheduleSave() {
        saveTask?.cancel()
        saveTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 700_000_000)
            if !Task.isCancelled { self?.save() }
        }
    }

    func save() {
        let toSave = consoles.map { c -> Console in
            var c = c; c.sql = sqlBuffer[c.id] ?? c.sql; return c
        }
        guard let data = try? JSONEncoder().encode(toSave),
              let json = String(data: data, encoding: .utf8) else { return }
        let api = self.api
        Task.detached(priority: .utility) { _ = api.dbConsolesSave(json: json) }
    }
}
