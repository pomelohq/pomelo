package services

import (
	"bufio"
	"strconv"
	"strings"
	"time"
)

// GitDiffLines reports which 1-based lines of the working file differ from HEAD,
// classified for a change gutter: "added", "modified", or "deleted" (the latter
// keyed by the surviving line the removal sits after).
type GitDiffLines struct {
	Added    []int `json:"added"`
	Modified []int `json:"modified"`
	Deleted  []int `json:"deleted"`
}

func GitDiffForFile(wt, relPath string) GitDiffLines {
	res := GitDiffLines{Added: []int{}, Modified: []int{}, Deleted: []int{}}
	out, err := RunTimeout(10*time.Second, wt, "git", "diff", "--no-color", "-U0", "HEAD", "--", relPath)
	if err != nil {
		return res
	}
	sc := bufio.NewScanner(strings.NewReader(string(out)))
	sc.Buffer(make([]byte, 0, 64*1024), 8*1024*1024)
	for sc.Scan() {
		line := sc.Text()
		if !strings.HasPrefix(line, "@@") {
			continue
		}
		// @@ -oldStart[,oldLen] +newStart[,newLen] @@
		fields := strings.Fields(line)
		if len(fields) < 3 {
			continue
		}
		oldStart, oldLen := parseHunkRange(fields[1]) // "-a,b"
		newStart, newLen := parseHunkRange(fields[2]) // "+c,d"
		_ = oldStart
		switch {
		case oldLen == 0 && newLen > 0:
			for i := 0; i < newLen; i++ {
				res.Added = append(res.Added, newStart+i)
			}
		case newLen == 0 && oldLen > 0:
			mark := newStart
			if mark < 1 {
				mark = 1
			}
			res.Deleted = append(res.Deleted, mark)
		default:
			for i := 0; i < newLen; i++ {
				res.Modified = append(res.Modified, newStart+i)
			}
		}
	}
	return res
}

// parseHunkRange decodes a hunk side like "+12,3" or "-8" into (start, length).
func parseHunkRange(s string) (start, length int) {
	s = strings.TrimLeft(s, "+-")
	if comma := strings.IndexByte(s, ','); comma >= 0 {
		start, _ = strconv.Atoi(s[:comma])
		length, _ = strconv.Atoi(s[comma+1:])
	} else {
		start, _ = strconv.Atoi(s)
		length = 1
	}
	return start, length
}
