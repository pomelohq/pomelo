package services

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"

	"github.com/pomelohq/pomelo/internal/paths"
)

func nmStoreDir(repo, lockHash string) string {
	return filepath.Join(paths.StateDir(), "nm-store", repo, lockHash, "node_modules")
}

func nmStoreRoot() string { return filepath.Join(paths.StateDir(), "nm-store") }

type NMStoreEntry struct {
	Repo  string `json:"repo"`
	Hash  string `json:"hash"`
	Bytes int64  `json:"bytes"`
	MTime int64  `json:"mtime"`
}

func MainLockHash(projectRoot, repo, defaultBranch string) string {
	return lockHash(RepoWorktreePath(projectRoot, repo, defaultBranch, true))
}

// LockHash returns the lockfile hash for a repo worktree (empty if none), used to
// match a workspace's repo to a cached node_modules snapshot.
func LockHash(worktree string) string { return lockHash(worktree) }

func nmIndexPath() string { return filepath.Join(nmStoreRoot(), ".index.json") }

func loadNMIndex() map[string]NMStoreEntry {
	idx := map[string]NMStoreEntry{}
	if data, err := os.ReadFile(nmIndexPath()); err == nil {
		var arr []NMStoreEntry
		if json.Unmarshal(data, &arr) == nil {
			for _, e := range arr {
				idx[e.Repo+"/"+e.Hash] = e
			}
		}
	}
	return idx
}

func saveNMIndex(entries []NMStoreEntry) {
	if data, err := json.Marshal(entries); err == nil {
		_ = os.WriteFile(nmIndexPath(), data, 0o644)
	}
}

// NMStoreEntries lists the cached node_modules and their sizes. Sizes are read
// from a persisted index and only recomputed (du) for a cache whose dir mtime
// changed — so the common read is cheap instead of walking every node_modules.
func NMStoreEntries() []NMStoreEntry {
	root := nmStoreRoot()
	repos, err := os.ReadDir(root)
	if err != nil {
		return nil
	}
	idx := loadNMIndex()
	var out []NMStoreEntry
	dirty := false
	for _, repo := range repos {
		if !repo.IsDir() {
			continue
		}
		hashes, err := os.ReadDir(filepath.Join(root, repo.Name()))
		if err != nil {
			continue
		}
		for _, h := range hashes {
			if !h.IsDir() {
				continue
			}
			p := filepath.Join(root, repo.Name(), h.Name())
			var mtime int64
			if fi, err := os.Stat(p); err == nil {
				mtime = fi.ModTime().Unix()
			}
			key := repo.Name() + "/" + h.Name()
			if c, ok := idx[key]; ok && c.MTime == mtime && c.Bytes > 0 {
				out = append(out, c)
				continue
			}
			dirty = true
			out = append(out, NMStoreEntry{Repo: repo.Name(), Hash: h.Name(),
				Bytes: dirSizeKB(p) * 1024, MTime: mtime})
		}
	}
	if dirty || len(out) != len(idx) {
		saveNMIndex(out)
	}
	return out
}

func dirSizeKB(path string) int64 {
	out, err := exec.Command("du", "-sk", path).Output()
	if err != nil {
		return 0
	}
	f := strings.Fields(string(out))
	if len(f) == 0 {
		return 0
	}
	n, _ := strconv.ParseInt(f[0], 10, 64)
	return n
}

func RemoveNMStoreEntry(repo, hash string) error {
	if repo == "" || hash == "" || strings.ContainsAny(repo+hash, "/\\") || strings.Contains(repo, "..") || strings.Contains(hash, "..") {
		return os.ErrInvalid
	}
	if err := os.RemoveAll(filepath.Join(nmStoreRoot(), repo, hash)); err != nil {
		return err
	}
	kept := make([]NMStoreEntry, 0)
	for k, e := range loadNMIndex() {
		if k != repo+"/"+hash {
			kept = append(kept, e)
		}
	}
	saveNMIndex(kept)
	return nil
}

func lockHash(worktree string) string {
	for _, name := range []string{"yarn.lock", "package-lock.json"} {
		if data, err := os.ReadFile(filepath.Join(worktree, name)); err == nil {
			sum := sha256.Sum256(data)
			return hex.EncodeToString(sum[:])[:16]
		}
	}
	return ""
}

func EnsureNodeModulesFromStore(projectRoot, repo, worktree, defaultBranch string) bool {
	dst := filepath.Join(worktree, "node_modules")
	if DirExists(dst) {
		return false
	}
	h := lockHash(worktree)
	if h == "" {
		return false
	}
	store := nmStoreDir(repo, h)
	if !DirExists(store) {
		mainWt := RepoWorktreePath(projectRoot, repo, defaultBranch, true)
		if lockHash(mainWt) != h || !DirExists(filepath.Join(mainWt, "node_modules")) {
			return false
		}
		_ = os.MkdirAll(filepath.Dir(store), 0o755)
		if err := cowCopy(filepath.Join(mainWt, "node_modules"), store); err != nil {
			return false
		}
	}
	return cowCopy(store, dst) == nil
}

func SnapshotNodeModules(repo, worktree string) {
	src := filepath.Join(worktree, "node_modules")
	if !DirExists(src) {
		return
	}
	h := lockHash(worktree)
	if h == "" {
		return
	}
	store := nmStoreDir(repo, h)
	if DirExists(store) {
		return
	}
	_ = os.MkdirAll(filepath.Dir(store), 0o755)
	_ = cowCopy(src, store)
}

func cowCopy(src, dst string) error {
	_ = os.RemoveAll(dst)
	var cow []string
	if runtime.GOOS == "darwin" {
		cow = []string{"cp", "-c", "-R", src, dst}
	} else {
		cow = []string{"cp", "--reflink=auto", "-a", src, dst}
	}
	if exec.Command(cow[0], cow[1:]...).Run() == nil {
		return nil
	}
	_ = os.RemoveAll(dst)
	return exec.Command("cp", "-R", src, dst).Run()
}
