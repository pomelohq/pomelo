import Foundation

@MainActor
final class GitViewModel: ObservableObject {
    struct Change: Decodable, Identifiable, Hashable {
        var path = ""; var orig = ""; var index = " "; var worktree = " "
        var staged = false; var unstaged = false; var untracked = false
        var id: String { path }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            path = try c.decodeIfPresent(String.self, forKey: .path) ?? ""
            orig = try c.decodeIfPresent(String.self, forKey: .orig) ?? ""
            index = try c.decodeIfPresent(String.self, forKey: .index) ?? " "
            worktree = try c.decodeIfPresent(String.self, forKey: .worktree) ?? " "
            staged = try c.decodeIfPresent(Bool.self, forKey: .staged) ?? false
            unstaged = try c.decodeIfPresent(Bool.self, forKey: .unstaged) ?? false
            untracked = try c.decodeIfPresent(Bool.self, forKey: .untracked) ?? false
        }
        enum K: String, CodingKey { case path, orig, index, worktree, staged, unstaged, untracked }

        // Single-letter badge for the row, biased to the worktree side the user acts on.
        var badge: String {
            if untracked { return "U" }
            let code = worktree != " " ? worktree : index
            return code == " " ? "?" : code
        }
    }

    struct RepoStatus: Decodable, Identifiable {
        var repo = ""; var branch = ""; var ahead = 0; var behind = 0
        var changes: [Change] = []
        var id: String { repo }
        init(from d: Decoder) throws {
            let c = try d.container(keyedBy: K.self)
            repo = try c.decodeIfPresent(String.self, forKey: .repo) ?? ""
            branch = try c.decodeIfPresent(String.self, forKey: .branch) ?? ""
            ahead = try c.decodeIfPresent(Int.self, forKey: .ahead) ?? 0
            behind = try c.decodeIfPresent(Int.self, forKey: .behind) ?? 0
            changes = try c.decodeIfPresent([Change].self, forKey: .changes) ?? []
        }
        enum K: String, CodingKey { case repo, branch, ahead, behind, changes }

        var staged: [Change] { changes.filter { $0.staged } }
        var unstaged: [Change] { changes.filter { $0.unstaged } }
        var isClean: Bool { changes.isEmpty }
    }

    private struct Payload: Decodable { var repos: [RepoStatus] = [] }

    @Published private(set) var repos: [RepoStatus] = []
    @Published private(set) var loading = true
    @Published var busy = false
    @Published var lastError = ""
    @Published var commitMessage: [String: String] = [:]

    let branch: String
    let isMain: Bool
    private let api: GitAPI

    init(branch: String, isMain: Bool, api: GitAPI = PomCore.shared) {
        self.branch = branch; self.isMain = isMain; self.api = api
    }

    var totalChanges: Int { repos.reduce(0) { $0 + $1.changes.count } }

    func load() async {
        let d = await api.call { $0.gitStatusData(branch: self.branch, isMain: self.isMain) }
        if let p = PomJSON.decode(Payload.self, from: d) { repos = p.repos }
        loading = false
    }

    private func run(_ work: @escaping (GitAPI) -> Data) async {
        busy = true; lastError = ""
        let d = await api.call(work)
        if let r = PomJSON.decode(OkResult.self, from: d), !r.ok {
            lastError = r.error.isEmpty ? "operation failed" : r.error
        }
        await load()
        busy = false
    }

    func stage(_ repo: String, _ paths: [String]) async {
        await run { $0.gitStage(branch: self.branch, repo: repo, isMain: self.isMain, paths: paths) }
    }
    func unstage(_ repo: String, _ paths: [String]) async {
        await run { $0.gitUnstage(branch: self.branch, repo: repo, isMain: self.isMain, paths: paths) }
    }
    func discard(_ repo: String, _ paths: [String]) async {
        await run { $0.gitDiscard(branch: self.branch, repo: repo, isMain: self.isMain, paths: paths) }
    }
    func stageAll(_ repo: String) async { await stage(repo, []) }

    func commit(_ repo: String) async {
        let msg = (commitMessage[repo] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !msg.isEmpty else { lastError = "Enter a commit message"; return }
        await run { $0.gitCommit(branch: self.branch, repo: repo, isMain: self.isMain, message: msg) }
        if lastError.isEmpty { commitMessage[repo] = "" }
    }
    func push(_ repo: String) async {
        await run { $0.gitPush(branch: self.branch, repo: repo, isMain: self.isMain) }
    }
}

struct OkResult: Decodable {
    var ok = false; var error = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? false
        error = try c.decodeIfPresent(String.self, forKey: .error) ?? ""
    }
    enum K: String, CodingKey { case ok, error }
}
