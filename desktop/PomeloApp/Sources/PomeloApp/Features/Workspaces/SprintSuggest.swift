import Foundation

// Ranking logic for the Create-workspace ticket suggestions, extracted from the
// view so it is unit-testable (MVVM): drop tickets whose branch already exists,
// honour the "only mine" filter, fuzzy-rank against the typed query, and sort
// mine-first then by score then key.
enum SprintSuggest {
    static func rank(_ sprint: [SprintIssue], existing: Set<String>, query: String, onlyMine: Bool) -> [SprintIssue] {
        let q = query.trimmingCharacters(in: .whitespaces)
        let avail = sprint.filter { !existing.contains($0.key.uppercased()) && (!onlyMine || $0.mine) }
        let scored: [(iss: SprintIssue, score: Int)] = avail.compactMap { iss in
            if q.isEmpty { return (iss, 0) }
            let s = [Fuzzy.score(q, iss.key), Fuzzy.score(q, iss.summary)].compactMap { $0 }.max()
            return s.map { (iss, $0) }
        }
        return scored.sorted {
            $0.iss.mine != $1.iss.mine ? ($0.iss.mine && !$1.iss.mine)
                : ($0.score != $1.score ? $0.score > $1.score : $0.iss.key < $1.iss.key)
        }.map(\.iss)
    }
}
