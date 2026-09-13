package services

import "strings"

var fsWatchIgnoredDirs = map[string]bool{
	".git": true, ".pom": true, "node_modules": true, ".ddata": true,
}

func FSWatchIgnoredPath(path string) bool {
	if path == "" {
		return false
	}
	for seg := range strings.SplitSeq(path, "/") {
		if fsWatchIgnoredDirs[seg] {
			return true
		}
	}
	return false
}
