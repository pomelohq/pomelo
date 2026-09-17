package core

import (
	"bufio"
	"encoding/json"
	"strconv"
	"strings"
	"time"

	"github.com/pomelohq/pomelo/internal/services"
)

type SearchLine struct {
	Line  int    `json:"line"`
	Text  string `json:"text"`
	Match bool   `json:"match"`
}

// SearchBlock is one contiguous code excerpt (a run of matched + surrounding context lines) within a file, like a
// section of Zed's project-search multibuffer.
type SearchBlock struct {
	Repo  string       `json:"repo"`
	Path  string       `json:"path"`
	Lines []SearchLine `json:"lines"`
}

// SearchFiles does a fixed-string, case-insensitive content search across every materialized repo worktree in the
// workspace, returning matched lines plus two lines of surrounding context grouped into excerpt blocks (Zed-style).
// Uses `git grep` so no external ripgrep dependency is needed. The query is passed as a separate arg, never a shell
// string.
func (s *Server) SearchFiles(branch, query string, isMain bool) []byte {
	empty := []byte(`{"blocks":[],"truncated":false}`)
	query = strings.TrimSpace(query)
	cfg := s.cfg()
	if query == "" || cfg == nil || s.WorkspaceRoot == "" {
		return empty
	}

	const maxMatches = 400
	blocks := make([]SearchBlock, 0, 32)
	matchCount := 0
	truncated := false

	for _, repo := range cfg.RepoOrder {
		if truncated {
			break
		}
		wt := repoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
		if !dirExists(wt) {
			continue
		}
		out, _ := services.RunTimeout(
			15*time.Second, wt,
			"git", "grep", "--heading", "--no-color", "-n", "-I", "-F", "-i", "-C", "2", "-e", query, "--",
		)

		sc := bufio.NewScanner(strings.NewReader(string(out)))
		sc.Buffer(make([]byte, 0, 64*1024), 8*1024*1024)

		path := ""
		var cur *SearchBlock
		flush := func() {
			if cur != nil && len(cur.Lines) > 0 {
				blocks = append(blocks, *cur)
			}
			cur = nil
		}

		for sc.Scan() {
			line := sc.Text()
			switch {
			case line == "": // blank line separates files
				flush()
			case line == "--": // git grep separates non-adjacent blocks within one file
				flush()
			default:
				// A body line is `<lineno>:<text>` (match) or `<lineno>-<text>` (context). Anything else with
				// --heading is the filename heading = a new file (don't rely on blank lines to detect it).
				num, sep, text, ok := splitGrepLine(line)
				if !ok {
					flush()
					path = line
					continue
				}
				if cur == nil {
					cur = &SearchBlock{Repo: repo, Path: path}
				}
				isMatch := sep == ':'
				if len(text) > 500 {
					text = text[:500]
				}
				cur.Lines = append(cur.Lines, SearchLine{Line: num, Text: text, Match: isMatch})
				if isMatch {
					matchCount++
					if matchCount >= maxMatches {
						truncated = true
					}
				}
			}
			if truncated {
				break
			}
		}
		flush()
	}

	b, _ := json.Marshal(map[string]any{"blocks": blocks, "truncated": truncated})
	return b
}

// splitGrepLine parses a `git grep -n` body line of the form `<lineno>:<text>` (a match) or `<lineno>-<text>`
// (context). Returns the line number, the separator byte, the text, and whether the parse succeeded.
func splitGrepLine(line string) (num int, sep byte, text string, ok bool) {
	i := 0
	for i < len(line) && line[i] >= '0' && line[i] <= '9' {
		i++
	}
	if i == 0 || i >= len(line) || (line[i] != ':' && line[i] != '-') {
		return 0, 0, "", false
	}
	n, err := strconv.Atoi(line[:i])
	if err != nil {
		return 0, 0, "", false
	}
	return n, line[i], line[i+1:], true
}
