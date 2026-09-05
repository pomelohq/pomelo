package core

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type FileEntry struct {
	Repo  string `json:"repo"`
	Path  string `json:"path"`
	IsDir bool   `json:"is_dir"`
	Size  int64  `json:"size,omitempty"`
}

var skipDirNames = map[string]bool{
	".git": true, ".pom": true, "node_modules": true, ".ddata": true,
}

func (s *Server) ListWorkspaceFiles(branch string, isMain bool) []byte {
	root := s.workspaceRoot(branch, isMain)
	if root == "" {
		return []byte(`[]`)
	}
	repos := s.workspaceRepoDirs(root)
	var out []FileEntry
	for _, repo := range repos {
		repoRoot := filepath.Join(root, repo)
		filepath.WalkDir(repoRoot, func(p string, d os.DirEntry, err error) error {
			if err != nil || p == repoRoot {
				return nil
			}
			if d.IsDir() && skipDirNames[d.Name()] {
				return filepath.SkipDir
			}
			rel, err := filepath.Rel(repoRoot, p)
			if err != nil {
				return nil
			}
			var size int64
			if !d.IsDir() {
				if info, err := d.Info(); err == nil {
					size = info.Size()
				}
			}
			out = append(out, FileEntry{Repo: repo, Path: filepath.ToSlash(rel), IsDir: d.IsDir(), Size: size})
			return nil
		})
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Repo != out[j].Repo {
			return out[i].Repo < out[j].Repo
		}
		return out[i].Path < out[j].Path
	})
	b, _ := json.Marshal(out)
	return b
}

func (s *Server) workspaceRepoDirs(root string) []string {
	entries, err := os.ReadDir(root)
	if err != nil {
		return nil
	}
	var names []string
	for _, e := range entries {
		if e.IsDir() && !skipDirNames[e.Name()] && !strings.HasPrefix(e.Name(), ".") {
			names = append(names, e.Name())
		}
	}
	sort.Strings(names)
	return names
}

type FileContent struct {
	Repo     string `json:"repo"`
	Path     string `json:"path"`
	MimeType string `json:"mime_type"`
	Text     string `json:"text,omitempty"`
	Base64   string `json:"base64,omitempty"`
	Size     int64  `json:"size"`
	Binary   bool   `json:"binary"`
}

func (s *Server) ReadFile(branch, repo, path string, isMain bool) []byte {
	if strings.Contains(path, "..") || filepath.IsAbs(path) {
		return []byte(`{"error":"bad path"}`)
	}
	root := s.workspaceRoot(branch, isMain)
	if root == "" {
		return []byte(`{"error":"no workspace"}`)
	}
	repo = s.resolveRepoDir(repo)
	full := filepath.Join(root, repo, path)
	info, err := os.Stat(full)
	if err != nil || info.IsDir() {
		return []byte(`{"error":"not found"}`)
	}

	mime := mimeFromExt(path)
	fc := FileContent{Repo: repo, Path: path, MimeType: mime, Size: info.Size()}

	if strings.HasPrefix(mime, "image/") {
		b, err := os.ReadFile(full)
		if err != nil {
			return []byte(`{"error":"read failed"}`)
		}
		fc.Base64 = base64.StdEncoding.EncodeToString(b)
		out, _ := json.Marshal(fc)
		return out
	}

	head := make([]byte, 8192)
	n, _ := readFileHead(full, head)
	if bytes.IndexByte(head[:n], 0) >= 0 {
		fc.Binary = true
		out, _ := json.Marshal(fc)
		return out
	}

	b, err := os.ReadFile(full)
	if err != nil {
		return []byte(`{"error":"read failed"}`)
	}
	fc.Text = string(b)
	out, _ := json.Marshal(fc)
	return out
}

func readFileHead(path string, buf []byte) (int, error) {
	f, err := os.Open(path)
	if err != nil {
		return 0, err
	}
	defer f.Close()
	n, err := f.Read(buf)
	if err != nil && n == 0 {
		return 0, nil
	}
	return n, nil
}

func mimeFromExt(path string) string {
	if t := mimeTypeByExtension(filepath.Ext(path)); t != "" {
		return t
	}
	return "application/octet-stream"
}

func mimeTypeByExtension(ext string) string {
	switch strings.ToLower(ext) {
	case ".png":
		return "image/png"
	case ".jpg", ".jpeg":
		return "image/jpeg"
	case ".gif":
		return "image/gif"
	case ".svg":
		return "image/svg+xml"
	case ".webp":
		return "image/webp"
	case ".bmp":
		return "image/bmp"
	default:
		return ""
	}
}
