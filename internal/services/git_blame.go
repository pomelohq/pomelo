package services

import (
	"bufio"
	"strconv"
	"strings"
	"time"
)

// BlameLine is the authorship of one line, as needed for an inline blame annotation.
type BlameLine struct {
	Author  string `json:"author"`
	Time    int64  `json:"time"` // author time, unix seconds
	Summary string `json:"summary"`
}

// GitBlame returns per-line authorship for the working copy of relPath, indexed 0-based by the
// final (current-file) line number. Uncommitted lines come back with an empty author.
func GitBlame(wt, relPath string) []BlameLine {
	out, err := RunTimeout(20*time.Second, wt, "git", "blame", "--porcelain", "--", relPath)
	if err != nil {
		return nil
	}
	type meta struct {
		author  string
		time    int64
		summary string
	}
	metas := map[string]*meta{}
	var lines []BlameLine
	var sha string
	var final int

	sc := bufio.NewScanner(strings.NewReader(string(out)))
	sc.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)
	for sc.Scan() {
		line := sc.Text()
		switch {
		case isBlameHeader(line):
			f := strings.Fields(line)
			sha = f[0]
			final, _ = strconv.Atoi(f[2])
			if metas[sha] == nil {
				metas[sha] = &meta{}
			}
		case strings.HasPrefix(line, "author "):
			metas[sha].author = strings.TrimPrefix(line, "author ")
		case strings.HasPrefix(line, "author-time "):
			metas[sha].time, _ = strconv.ParseInt(strings.TrimPrefix(line, "author-time "), 10, 64)
		case strings.HasPrefix(line, "summary "):
			metas[sha].summary = strings.TrimPrefix(line, "summary ")
		case strings.HasPrefix(line, "\t"):
			m := metas[sha]
			for len(lines) < final {
				lines = append(lines, BlameLine{})
			}
			if final >= 1 && m != nil {
				author := m.author
				if author == "Not Committed Yet" {
					author = ""
				}
				lines[final-1] = BlameLine{Author: author, Time: m.time, Summary: m.summary}
			}
		}
	}
	return lines
}

// isBlameHeader matches a porcelain group header: a 40-hex sha followed by a space.
func isBlameHeader(line string) bool {
	if len(line) < 41 || line[40] != ' ' {
		return false
	}
	for i := 0; i < 40; i++ {
		c := line[i]
		if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')) {
			return false
		}
	}
	return true
}
