package forge

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"

	"github.com/pomelohq/pomelo/internal/diffparse"
	"github.com/pomelohq/pomelo/internal/httpx"
	"github.com/pomelohq/pomelo/internal/services"
)

func (s *Feature) handleChangedFiles(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	isMain := r.URL.Query().Get("is_main") == "true"
	if branch == "" || repo == "" {
		http.Error(w, "missing branch/repo", http.StatusBadRequest)
		return
	}
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	base := services.BaseRef(defBranch(s.cfg(), repo), wt)

	out, err := exec.Command("git", "-C", wt, "diff", "--name-status",
		"--find-renames", base+"...HEAD").Output()
	if err != nil {
		http.Error(w, "git diff failed: "+err.Error(), http.StatusInternalServerError)
		return
	}
	type entry struct {
		Path    string `json:"path"`
		Status  string `json:"status"`
		OldPath string `json:"old_path,omitempty"`
	}
	var files []entry
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if line == "" {
			continue
		}
		fields := strings.Split(line, "\t")
		if len(fields) < 2 {
			continue
		}
		st := fields[0]
		if strings.HasPrefix(st, "R") && len(fields) >= 3 {
			files = append(files, entry{Path: fields[2], Status: "R", OldPath: fields[1]})
			continue
		}
		files = append(files, entry{Path: fields[len(fields)-1], Status: st[:1]})
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"base": base, "files": files})
}

func (s *Feature) handleFileContent(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	isMain := r.URL.Query().Get("is_main") == "true"
	path := r.URL.Query().Get("path")
	ref := r.URL.Query().Get("ref")
	if branch == "" || repo == "" || path == "" || ref == "" {
		http.Error(w, "missing branch/repo/path/ref", http.StatusBadRequest)
		return
	}
	if strings.Contains(path, "..") {
		http.Error(w, "path contains ..", http.StatusBadRequest)
		return
	}
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	var gitRef string
	switch ref {
	case "branch":
		gitRef = "HEAD"
	case "base":
		gitRef = services.BaseRef(defBranch(s.cfg(), repo), wt)
	default:
		http.Error(w, "ref must be branch|base", http.StatusBadRequest)
		return
	}
	out, err := exec.Command("git", "-C", wt, "show", gitRef+":"+path).Output()
	if err != nil {
		http.Error(w, "file not found at ref", http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_, _ = w.Write(out)
}

func (s *Feature) handlePRDetail(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	if branch == "" || repo == "" {
		httpx.Err(w, http.StatusBadRequest, "missing branch/repo")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(s.PRDetail(branch, repo, r.URL.Query().Get("is_main") == "true"))
}

func (s *Feature) PRDetail(branch, repo string, isMain bool) []byte {
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	return cachedConv("detail:"+wt, func() []byte { return fetchPRDetail(wt) })
}

func fetchPRDetail(wt string) []byte {
	owner, name, ok := ownerRepo(wt)
	head := services.CurrentBranch(wt)
	if !ok || head == "" {
		body, _ := json.Marshal(map[string]any{"pr": nil})
		return body
	}
	q := fmt.Sprintf(`query { repository(owner: %q, name: %q) { pullRequests(headRefName: %q, first: 1, states: [OPEN, MERGED, CLOSED], orderBy: {field: UPDATED_AT, direction: DESC}) { nodes {
%s      body
        labels(first: 20) { nodes { name color } }
        reviewRequests(first: 20) { nodes { requestedReviewer { __typename ... on User { login } } } }
        comments(first: 50) { nodes { author { login avatarUrl } body createdAt } }
        reviews(first: 50) { nodes { databaseId state body submittedAt author { login avatarUrl } } }
      } } } }`, owner, name, head, prNodeFields)
	out, err := gqlQuery(context.Background(), q)
	var r struct {
		Data struct {
			Repository *struct {
				PullRequests struct {
					Nodes []gqlPR `json:"nodes"`
				} `json:"pullRequests"`
			} `json:"repository"`
		} `json:"data"`
	}
	if err == nil && json.Unmarshal(out, &r) == nil && r.Data.Repository != nil && len(r.Data.Repository.PullRequests.Nodes) > 0 {
		body, _ := json.Marshal(map[string]any{"pr": r.Data.Repository.PullRequests.Nodes[0].toGH()})
		return body
	}
	body, _ := json.Marshal(map[string]any{"pr": nil})
	return body
}

func (s *Feature) handlePRReviewComments(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	if branch == "" || repo == "" {
		httpx.Err(w, http.StatusBadRequest, "missing branch/repo")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(s.PRComments(branch, repo, r.URL.Query().Get("is_main") == "true"))
}

func (s *Feature) PRComments(branch, repo string, isMain bool) []byte {
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	return cachedConv("comments:"+wt, func() []byte { return fetchPRComments(wt) })
}

func fetchPRComments(wt string) []byte {
	empty := []byte(`{"comments":[]}`)
	owner, name, ok := ownerRepo(wt)
	if !ok {
		return empty
	}
	head := services.CurrentBranch(wt)
	if head == "" {
		return empty
	}
	ctx := context.Background()
	numRaw, err := restGET(ctx, fmt.Sprintf("repos/%s/%s/pulls?head=%s:%s&state=all&per_page=1", owner, name, owner, head))
	if err != nil {
		return empty
	}
	var pulls []struct {
		Number int `json:"number"`
	}
	if json.Unmarshal(numRaw, &pulls) != nil || len(pulls) == 0 {
		return empty
	}
	raw, err := restGET(ctx, fmt.Sprintf("repos/%s/%s/pulls/%d/comments?per_page=100", owner, name, pulls[0].Number))
	if err != nil {
		return empty
	}
	var apiComments []struct {
		User struct {
			Login     string `json:"login"`
			AvatarURL string `json:"avatar_url"`
		} `json:"user"`
		Body         string `json:"body"`
		Path         string `json:"path"`
		Line         *int   `json:"line"`
		OriginalLine *int   `json:"original_line"`
		DiffHunk     string `json:"diff_hunk"`
		CreatedAt    string `json:"created_at"`
		ReviewID     *int64 `json:"pull_request_review_id"`
	}
	if json.Unmarshal(raw, &apiComments) != nil {
		return empty
	}
	type outComment struct {
		User      string           `json:"user"`
		AvatarURL string           `json:"avatarUrl,omitempty"`
		Body      string           `json:"body"`
		Path      string           `json:"path"`
		Line      *int             `json:"line"`
		DiffHunk  string           `json:"diffHunk"`
		HunkLines []diffparse.Line `json:"hunkLines"`
		CreatedAt string           `json:"createdAt"`
		ReviewID  *int64           `json:"reviewId"`
	}
	out := make([]outComment, 0, len(apiComments))
	for _, c := range apiComments {
		line := c.Line
		if line == nil {
			line = c.OriginalLine
		}
		out = append(out, outComment{User: c.User.Login, AvatarURL: c.User.AvatarURL, Body: c.Body, Path: c.Path, Line: line, DiffHunk: c.DiffHunk, HunkLines: diffparse.ParseHunk(c.DiffHunk), CreatedAt: c.CreatedAt, ReviewID: c.ReviewID})
	}
	b, _ := json.Marshal(map[string]any{"comments": out})
	return b
}

const maxDiffBytes = 512 * 1024

func (s *Feature) handleDiff(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	if branch == "" || repo == "" {
		httpx.Err(w, http.StatusBadRequest, "missing branch/repo")
		return
	}
	out, err := s.Diff(branch, repo, r.URL.Query().Get("is_main") == "true")
	if err != nil {
		httpx.Err(w, http.StatusInternalServerError, "git diff failed")
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_, _ = w.Write(out)
}

var (
	headFetchMu sync.Mutex
	headFetchAt = map[string]time.Time{}
)

// fetchPRHead refreshes origin/<head> in the background (throttled) so the next
// Files/Commits open reflects newly-pushed commits without blocking this one.
func fetchPRHead(wt, head string) {
	headFetchMu.Lock()
	if time.Since(headFetchAt[wt]) < 45*time.Second {
		headFetchMu.Unlock()
		return
	}
	headFetchAt[wt] = time.Now()
	headFetchMu.Unlock()
	go func() { _, _ = services.RunTimeout(8*time.Second, wt, "git", "fetch", "origin", head) }()
}

// prHeadRef returns the pushed PR head (origin/<branch>) so Files/Commits match
// the PR on GitHub even when the local worktree is behind the pushed branch.
// Local-only work belongs to the "Local changes" view instead.
func prHeadRef(wt string) string {
	if head := services.CurrentBranch(wt); head != "" {
		fetchPRHead(wt, head)
		if exec.Command("git", "-C", wt, "rev-parse", "--verify", "--quiet", "origin/"+head).Run() == nil {
			return "origin/" + head
		}
	}
	return "HEAD"
}

func (s *Feature) Diff(branch, repo string, isMain bool) ([]byte, error) {
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	base := services.BaseRef(defBranch(s.cfg(), repo), wt)
	out, err := services.RunTimeout(10*time.Second, wt, "git", "diff", "-M", base+"..."+prHeadRef(wt))
	if err != nil {
		return nil, err
	}
	if len(out) > maxDiffBytes {
		return append(out[:maxDiffBytes], []byte("\n… diff truncated (too large) — open on GitHub for the full view\n")...), nil
	}
	return out, nil
}

type prCacheEntry struct {
	body []byte
	at   time.Time
}

var (
	prCacheMu sync.Mutex
	prCache   = map[string]prCacheEntry{}
)

func (s *Feature) handlePRStatus(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	isMain := r.URL.Query().Get("is_main") == "true"
	if branch == "" || repo == "" {
		http.Error(w, "missing branch/repo", http.StatusBadRequest)
		return
	}
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)

	key := wt
	prCacheMu.Lock()
	if hit, ok := prCache[key]; ok && time.Since(hit.at) < 30*time.Second {
		body := hit.body
		prCacheMu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write(body)
		return
	}
	prCacheMu.Unlock()

	if head := services.CurrentBranch(wt); head != "" {
		if p, ok := prPairFor(repo, wt); ok {
			warmPRs([]prPair{p})
		}
		if pr, known := cachedPR(repo, head); known && pr != nil {
			body, _ := json.Marshal(map[string]any{"pr": pr})
			prCacheMu.Lock()
			prCache[key] = prCacheEntry{body: body, at: time.Now()}
			prCacheMu.Unlock()
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write(body)
			return
		}
	}

	body, _ := json.Marshal(map[string]any{"pr": nil})
	prCacheMu.Lock()
	prCache[key] = prCacheEntry{body: body, at: time.Now()}
	prCacheMu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(body)
}

func (s *Feature) handleRepoCommits(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	if branch == "" || repo == "" {
		httpx.Err(w, http.StatusBadRequest, "missing branch/repo")
		return
	}
	httpx.Write(w, s.RepoCommits(branch, repo, r.URL.Query().Get("base"), r.URL.Query().Get("is_main") == "true"))
}

func (s *Feature) RepoCommits(branch, repo, baseOverride string, isMain bool) map[string]any {
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	base := services.BaseRef(defBranch(s.cfg(), repo), wt)
	if baseOverride != "" {
		base = "origin/" + baseOverride
	}
	out, err := services.RunTimeout(6*time.Second, wt, "git", "log",
		base+".."+prHeadRef(wt), "--format=%h%x1f%s%x1f%an%x1f%cr", "-n", "100")
	commits := []map[string]string{}
	if err == nil {
		for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
			if line == "" {
				continue
			}
			p := strings.SplitN(line, "\x1f", 4)
			if len(p) == 4 {
				commits = append(commits, map[string]string{"hash": p[0], "subject": p[1], "author": p[2], "date": p[3]})
			}
		}
	}
	return map[string]any{"commits": commits}
}

// CommitDiff returns the patch a single commit introduced.
func (s *Feature) CommitDiff(branch, repo, sha string, isMain bool) ([]byte, error) {
	if !isHexSHA(sha) {
		return nil, fmt.Errorf("bad commit sha %q", sha)
	}
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	// --format= drops the commit header; -M so a rename reads as one entry.
	out, err := services.RunTimeout(10*time.Second, wt, "git", "show", "-M", "--format=", sha)
	if err != nil {
		return nil, err
	}
	if len(out) > maxDiffBytes {
		return append(out[:maxDiffBytes], []byte("\n… diff truncated (too large)\n")...), nil
	}
	return out, nil
}

// The sha arrives from the UI and lands in an argv slot where git also accepts
// options, so anything but hex is rejected rather than escaped.
func isHexSHA(s string) bool {
	if len(s) < 7 || len(s) > 40 {
		return false
	}
	for _, r := range s {
		if (r < '0' || r > '9') && (r < 'a' || r > 'f') {
			return false
		}
	}
	return true
}

const (
	prTTL         = 150 * time.Second
	prErrCooldown = 30 * time.Second
)

func (s *Feature) handleAllPRs(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(s.AllPRs())
}

func (s *Feature) AllPRs() []byte {
	if s.cfg() == nil || s.WorkspaceRoot == "" {
		return []byte(`{}`)
	}
	type repoPR struct {
		Repo   string          `json:"repo"`
		Alias  string          `json:"alias"`
		PR     json.RawMessage `json:"pr"`
		Behind int             `json:"behind"`
		Ahead  int             `json:"ahead"`
	}
	known := make(map[string]bool, len(s.cfg().Repos))
	for n := range s.cfg().Repos {
		known[n] = true
	}
	workspaces := scanSkeletons(s.WorkspaceRoot, s.DefaultBranch, known, true)

	var pairs []prPair
	anyCold := false
	for _, ws := range workspaces {
		for _, repo := range ws.Repos {
			wt := services.RepoWorktreePath(s.WorkspaceRoot, repo.Name, ws.Branch, ws.IsMain)
			if st, err := os.Stat(wt); err != nil || !st.IsDir() {
				continue
			}
			p, ok := prPairFor(repo.Name, wt)
			if !ok {
				continue
			}
			pairs = append(pairs, p)
			if _, known := cachedPR(p.repo, p.head); !known {
				anyCold = true
			}
		}
	}
	_ = anyCold
	go warmPRs(pairs)

	type wsGroup struct {
		Prs      []repoPR `json:"prs"`
		Severity string   `json:"severity"`
	}
	out := map[string]wsGroup{}
	for _, ws := range workspaces {
		key := "ws:" + ws.Branch
		if ws.IsMain {
			key = "main:" + ws.Branch
		}
		var list []repoPR
		for _, repo := range ws.Repos {
			dir := s.cfg().Repos[repo.Name]
			if dir == nil {
				continue
			}
			alias := dir.Alias
			if alias == "" {
				alias = repo.Name
			}
			wt := services.RepoWorktreePath(s.WorkspaceRoot, repo.Name, ws.Branch, ws.IsMain)
			if st, err := os.Stat(wt); err != nil || !st.IsDir() {
				continue
			}
			head := services.CurrentBranch(wt)
			if head == "" {
				continue
			}
			if pr, known := cachedPR(repo.Name, head); known && pr != nil {
				ahead, behind := services.AheadBehind(defBranch(s.cfg(), repo.Name), wt)
				list = append(list, repoPR{Repo: repo.Name, Alias: alias, PR: pr, Behind: behind, Ahead: ahead})
			}
		}
		if len(list) > 0 {
			blobs := make([]json.RawMessage, len(list))
			for i, r := range list {
				blobs[i] = r.PR
			}
			out[key] = wsGroup{Prs: list, Severity: prSeverity(blobs)}
		}
	}
	b, _ := json.Marshal(out)
	return b
}

func (s *Feature) handleWorkspacePRs(w http.ResponseWriter, r *http.Request) {
	if s.cfg() == nil {
		http.Error(w, "no project config", http.StatusServiceUnavailable)
		return
	}
	branch := r.URL.Query().Get("branch")
	if branch == "" {
		http.Error(w, "missing branch", http.StatusBadRequest)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(s.WorkspacePRs(branch, r.URL.Query().Get("is_main") == "true"))
}

func (s *Feature) WorkspacePRs(branch string, isMain bool) []byte {
	if s.cfg() == nil {
		return []byte(`{"prs":[]}`)
	}
	type repoPR struct {
		Repo   string          `json:"repo"`
		Alias  string          `json:"alias"`
		PR     json.RawMessage `json:"pr"`
		Behind int             `json:"behind"`
		Ahead  int             `json:"ahead"`
	}

	type item struct {
		alias string
		wt    string
		pair  prPair
	}
	var items []item
	var pairs []prPair
	for _, name := range s.cfg().RepoOrder {
		dir, ok := s.cfg().Repos[name]
		if !ok {
			continue
		}
		alias := dir.Alias
		if alias == "" {
			alias = name
		}
		wt := services.RepoWorktreePath(s.WorkspaceRoot, name, branch, isMain)
		if st, err := os.Stat(wt); err != nil || !st.IsDir() {
			continue
		}
		p, ok := prPairFor(name, wt)
		if !ok {
			continue
		}
		items = append(items, item{alias, wt, p})
		pairs = append(pairs, p)
	}
	warmPRs(pairs)

	var out []repoPR
	for _, it := range items {
		if pr, known := cachedPR(it.pair.repo, it.pair.head); known && pr != nil {
			ahead, behind := services.AheadBehind(defBranch(s.cfg(), it.pair.repo), it.wt)
			out = append(out, repoPR{Repo: it.pair.repo, Alias: it.alias, PR: pr, Behind: behind, Ahead: ahead})
		}
	}
	blobs := make([]json.RawMessage, len(out))
	for i, r := range out {
		blobs[i] = r.PR
	}
	b, _ := json.Marshal(map[string]any{"prs": out, "severity": prSeverity(blobs)})
	return b
}

func (s *Feature) handleLocalChanges(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	if branch == "" {
		http.Error(w, "missing branch", http.StatusBadRequest)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(s.WorkspaceLocalChanges(branch, r.URL.Query().Get("is_main") == "true"))
}

func (s *Feature) WorkspaceLocalChanges(branch string, isMain bool) []byte {
	if s.cfg() == nil {
		return []byte(`{"repos":[]}`)
	}
	type repoChange struct {
		Repo       string `json:"repo"`
		Alias      string `json:"alias"`
		Files      int    `json:"files"`
		Insertions int    `json:"insertions"`
		Deletions  int    `json:"deletions"`
		Behind     int    `json:"behind"`
	}
	var out []repoChange
	for _, name := range s.cfg().RepoOrder {
		dir, ok := s.cfg().Repos[name]
		if !ok {
			continue
		}
		wt := services.RepoWorktreePath(s.WorkspaceRoot, name, branch, isMain)
		if st, err := os.Stat(wt); err != nil || !st.IsDir() {
			continue
		}
		base := services.UnpushedBase(defBranch(s.cfg(), name), wt)
		files, ins, del := services.LocalChangeStat(base, wt)
		// Reads the ref WarmLoop refreshes; the local stat is measured against a
		// merge-base, which a teammate's push cannot move.
		behind := services.UpstreamBehind(wt)
		// A repo with no local edits still matters when upstream moved ahead.
		if files == 0 && behind == 0 {
			continue
		}
		alias := dir.Alias
		if alias == "" {
			alias = name
		}
		out = append(out, repoChange{Repo: name, Alias: alias, Files: files, Insertions: ins, Deletions: del, Behind: behind})
	}
	b, _ := json.Marshal(map[string]any{"repos": out})
	return b
}

func (s *Feature) handleLocalDiff(w http.ResponseWriter, r *http.Request) {
	branch := r.URL.Query().Get("branch")
	repo := r.URL.Query().Get("repo")
	if branch == "" || repo == "" {
		httpx.Err(w, http.StatusBadRequest, "missing branch/repo")
		return
	}
	out, err := s.LocalDiff(branch, repo, r.URL.Query().Get("is_main") == "true")
	if err != nil {
		httpx.Err(w, http.StatusInternalServerError, "git diff failed")
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_, _ = w.Write(out)
}

func (s *Feature) LocalDiff(branch, repo string, isMain bool) ([]byte, error) {
	wt := services.RepoWorktreePath(s.WorkspaceRoot, repo, branch, isMain)
	base := services.UnpushedBase(defBranch(s.cfg(), repo), wt)
	out, err := services.RunTimeout(10*time.Second, wt, "git", "diff", "-M", base)
	if err != nil {
		return nil, err
	}
	for _, name := range services.UntrackedFiles(wt) {
		d, _ := services.RunTimeout(10*time.Second, wt, "git", "diff", "--no-index", "--", "/dev/null", name)
		out = append(out, d...)
	}
	if len(out) > maxDiffBytes {
		return append(out[:maxDiffBytes], []byte("\n… diff truncated (too large)\n")...), nil
	}
	return out, nil
}
