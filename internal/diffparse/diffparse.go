// Package diffparse turns a unified git diff into structured files/lines. Parsing
// the diff format is domain logic (ADR 0001) — the frontend only renders the result.
package diffparse

import (
	"encoding/json"
	"strconv"
	"strings"
)

type Kind string

const (
	KindContext Kind = "context"
	KindAdd     Kind = "add"
	KindDel     Kind = "del"
	KindHunk    Kind = "hunk"
)

type Line struct {
	ID   int    `json:"id"`
	Kind Kind   `json:"kind"`
	OldN *int   `json:"old_n,omitempty"`
	NewN *int   `json:"new_n,omitempty"`
	Text string `json:"text"`
}

type File struct {
	Path          string  `json:"path"`
	OldPath       *string `json:"old_path,omitempty"`
	Status        string  `json:"status"`
	Adds          int     `json:"adds"`
	Dels          int     `json:"dels"`
	Binary        bool    `json:"binary"`
	Lines         []Line  `json:"lines"`
	HeaderOldPath string  `json:"header_old_path"`
}

// ParseJSON parses raw git diff bytes and marshals the structured files. Emits `[]`
// (never null) so the Swift Decodable sees an array for the empty case.
func ParseJSON(raw []byte) []byte {
	b, err := json.Marshal(Parse(string(raw)))
	if err != nil {
		return []byte("[]")
	}
	return b
}

func Parse(text string) []File {
	files := []File{}
	var cur *File
	oldN, newN, lid := 0, 0, 0

	flush := func() {
		if cur != nil {
			files = append(files, *cur)
		}
		cur = nil
	}

	for _, raw := range strings.Split(text, "\n") {
		if strings.HasPrefix(raw, "diff --git ") {
			flush()
			a, b := gitHeaderPaths(strings.TrimPrefix(raw, "diff --git "))
			cur = &File{Path: b, Status: "M", HeaderOldPath: a, Lines: []Line{}}
			oldN, newN = 0, 0
			continue
		}
		if cur == nil {
			continue
		}
		switch {
		case strings.HasPrefix(raw, "new file"):
			cur.Status = "A"
			continue
		case strings.HasPrefix(raw, "deleted file"):
			cur.Status = "D"
			if cur.HeaderOldPath != "" {
				cur.Path = cur.HeaderOldPath
			}
			continue
		case strings.HasPrefix(raw, "rename from "):
			setOld(cur, strings.TrimPrefix(raw, "rename from "))
			cur.Status = "R"
			continue
		case strings.HasPrefix(raw, "rename to "):
			cur.Path = strings.TrimPrefix(raw, "rename to ")
			cur.Status = "R"
			continue
		case strings.HasPrefix(raw, "copy from "):
			setOld(cur, strings.TrimPrefix(raw, "copy from "))
			cur.Status = "C"
			continue
		case strings.HasPrefix(raw, "copy to "):
			cur.Path = strings.TrimPrefix(raw, "copy to ")
			cur.Status = "C"
			continue
		case strings.HasPrefix(raw, "Binary files"):
			cur.Binary = true
			continue
		case strings.HasPrefix(raw, "--- "):
			continue
		case strings.HasPrefix(raw, "+++ "):
			p := raw[4:]
			if p != "/dev/null" {
				cur.Path = strings.TrimPrefix(p, "b/")
			}
			continue
		case strings.HasPrefix(raw, "index "), strings.HasPrefix(raw, "similarity "), strings.HasPrefix(raw, "\\ "):
			continue
		case strings.HasPrefix(raw, "@@"):
			oldN, newN = hunkStart(raw)
			lid++
			cur.Lines = append(cur.Lines, Line{ID: lid, Kind: KindHunk, Text: raw})
			continue
		}
		// Trailing blank from the final newline, before any hunk: not a context line.
		if raw == "" && len(cur.Lines) == 0 {
			continue
		}
		lid++
		switch {
		case strings.HasPrefix(raw, "+"):
			n := newN
			cur.Lines = append(cur.Lines, Line{ID: lid, Kind: KindAdd, NewN: &n, Text: raw[1:]})
			cur.Adds++
			newN++
		case strings.HasPrefix(raw, "-"):
			o := oldN
			cur.Lines = append(cur.Lines, Line{ID: lid, Kind: KindDel, OldN: &o, Text: raw[1:]})
			cur.Dels++
			oldN++
		default:
			t := strings.TrimPrefix(raw, " ")
			o, n := oldN, newN
			cur.Lines = append(cur.Lines, Line{ID: lid, Kind: KindContext, OldN: &o, NewN: &n, Text: t})
			oldN++
			newN++
		}
	}
	flush()
	return files
}

func setOld(f *File, p string) { f.OldPath = &p }

// gitHeaderPaths splits a `diff --git a/x b/y` header. Cannot split on spaces —
// paths may contain them — so the boundary is the last " b/" leaving a well-formed `a/`.
func gitHeaderPaths(body string) (string, string) {
	s := []rune(body)
	var candidates []int
	if len(s) > 3 {
		for i := 0; i <= len(s)-3; i++ {
			if s[i] == ' ' && s[i+1] == 'b' && s[i+2] == '/' {
				candidates = append(candidates, i)
			}
		}
	}
	for i := len(candidates) - 1; i >= 0; i-- {
		c := candidates[i]
		left, right := string(s[:c]), string(s[c+1:])
		if strings.HasPrefix(left, "a/") && strings.HasPrefix(right, "b/") {
			return strings.TrimPrefix(left, "a/"), strings.TrimPrefix(right, "b/")
		}
	}
	return "", ""
}

func hunkStart(s string) (int, int) {
	old, new := 0, 0
	for _, tok := range strings.Fields(s[2:]) {
		if strings.HasPrefix(tok, "-") {
			old = leadingInt(tok[1:])
		}
		if strings.HasPrefix(tok, "+") {
			new = leadingInt(tok[1:])
			break
		}
	}
	return old, new
}

func leadingInt(s string) int {
	if i := strings.IndexByte(s, ','); i >= 0 {
		s = s[:i]
	}
	n, _ := strconv.Atoi(s)
	return n
}
