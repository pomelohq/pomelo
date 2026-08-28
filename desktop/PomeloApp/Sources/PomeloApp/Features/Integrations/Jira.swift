import SwiftUI

struct JiraIssue: Decodable, Equatable {
    var key: String = ""
    var summary: String = ""
    var status: String = ""
    var category: String = ""   // new | indeterminate | done
    var assignee: String = ""
    var url: String = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        category = try c.decodeIfPresent(String.self, forKey: .category) ?? ""
        assignee = try c.decodeIfPresent(String.self, forKey: .assignee) ?? ""
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
    }
    enum K: String, CodingKey { case key, summary, status, category, assignee, url }
}

struct JiraBoard: Decodable, Identifiable, Equatable, Hashable {
    var id: Int = 0
    var name: String = ""
    var type: String = ""
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        id = try c.decodeIfPresent(Int.self, forKey: .id) ?? 0
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        type = try c.decodeIfPresent(String.self, forKey: .type) ?? ""
    }
    enum K: String, CodingKey { case id, name, type }
}

struct SprintIssue: Decodable, Identifiable, Equatable {
    var key: String = ""
    var summary: String = ""
    var status: String = ""
    var assignee: String = ""
    var avatar: String = ""
    var sprint: String = ""
    var mine: Bool = false
    var id: String { key }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: K.self)
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        assignee = try c.decodeIfPresent(String.self, forKey: .assignee) ?? ""
        avatar = try c.decodeIfPresent(String.self, forKey: .avatar) ?? ""
        sprint = try c.decodeIfPresent(String.self, forKey: .sprint) ?? ""
        mine = try c.decodeIfPresent(Bool.self, forKey: .mine) ?? false
    }
    init(key: String = "", summary: String = "", status: String = "", assignee: String = "",
         avatar: String = "", sprint: String = "", mine: Bool = false) {
        self.key = key; self.summary = summary; self.status = status; self.assignee = assignee
        self.avatar = avatar; self.sprint = sprint; self.mine = mine
    }
    enum K: String, CodingKey { case key, summary, status, assignee, avatar, sprint, mine }
}

func jiraKey(_ branch: String) -> String? {
    let chars = Array(branch)
    var i = 0
    while i < chars.count {
        guard chars[i].isLetter else { i += 1; continue }
        var j = i
        while j < chars.count && chars[j].isLetter { j += 1 }
        if j < chars.count && chars[j] == "-" {
            var k = j + 1
            while k < chars.count && chars[k].isNumber { k += 1 }
            if k > j + 1 { return String(chars[i..<k]).uppercased() }
        }
        i = j + 1
    }
    return nil
}

extension JiraIssue {
    var color: Color {
        switch category {
        case "done": return Theme.ok
        case "indeterminate": return Theme.accent
        default: return Theme.fgMuted
        }
    }
}
