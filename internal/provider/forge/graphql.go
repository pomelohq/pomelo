package forge

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"sync"
	"time"

	"github.com/pomelohq/pomelo/internal/services"
)

type prPair struct{ repo, owner, name, head string }

type prHeadEntry struct {
	pr json.RawMessage
	at time.Time
}

var (
	prHeadMu    sync.Mutex
	prHeadCache = map[string]prHeadEntry{}
	prWarmMu    sync.Mutex
	prNextRetry time.Time
)

func prHeadKey(repo, head string) string { return repo + "\x00" + head }

func cachedPR(repo, head string) (raw json.RawMessage, known bool) {
	prHeadMu.Lock()
	defer prHeadMu.Unlock()
	e, ok := prHeadCache[prHeadKey(repo, head)]
	if !ok {
		return nil, false
	}
	return e.pr, true
}

func prPairFor(repo, wt string) (prPair, bool) {
	owner, name, ok := ownerRepo(wt)
	if !ok {
		return prPair{}, false
	}
	head := services.CurrentBranch(wt)
	if head == "" {
		return prPair{}, false
	}
	return prPair{repo: repo, owner: owner, name: name, head: head}, true
}

func warmPRs(pairs []prPair) {
	if len(pairs) == 0 {
		return
	}
	prHeadMu.Lock()
	if time.Now().Before(prNextRetry) {
		prHeadMu.Unlock()
		return
	}
	seen := map[string]bool{}
	var stale []prPair
	for _, p := range pairs {
		k := prHeadKey(p.repo, p.head)
		if seen[k] {
			continue
		}
		seen[k] = true
		if e, ok := prHeadCache[k]; !ok || time.Since(e.at) >= prTTL {
			stale = append(stale, p)
		}
	}
	prHeadMu.Unlock()
	if len(stale) == 0 {
		return
	}

	prWarmMu.Lock()
	defer prWarmMu.Unlock()
	prHeadMu.Lock()
	var todo []prPair
	for _, p := range stale {
		if e, ok := prHeadCache[prHeadKey(p.repo, p.head)]; !ok || time.Since(e.at) >= prTTL {
			todo = append(todo, p)
		}
	}
	prHeadMu.Unlock()
	if len(todo) == 0 {
		return
	}

	type gqlResp struct {
		Data map[string]*struct {
			PullRequests struct {
				Nodes []gqlPR `json:"nodes"`
			} `json:"pullRequests"`
		} `json:"data"`
	}
	now := time.Now()
	failed := false
	for start := 0; start < len(todo); start += prBatch {
		end := start + prBatch
		if end > len(todo) {
			end = len(todo)
		}
		chunk := todo[start:end]

		var b strings.Builder
		b.WriteString("query {\n")
		for i, p := range chunk {
			fmt.Fprintf(&b, "  p%d: repository(owner: %q, name: %q) { pullRequests(headRefName: %q, first: 1, states: [OPEN, MERGED], orderBy: {field: UPDATED_AT, direction: DESC}) { nodes {\n%s      } } }\n",
				i, p.owner, p.name, p.head, prNodeFields)
		}
		b.WriteString("}\n")

		out, err := gqlQuery(context.Background(), b.String())
		var resp gqlResp
		if (err != nil && len(out) == 0) || json.Unmarshal(out, &resp) != nil || resp.Data == nil {
			failed = true
			continue
		}
		prHeadMu.Lock()
		for i, p := range chunk {
			var raw json.RawMessage
			if node := resp.Data[fmt.Sprintf("p%d", i)]; node != nil && len(node.PullRequests.Nodes) > 0 {
				if enc, e := json.Marshal(node.PullRequests.Nodes[0].toGH()); e == nil {
					raw = enc
				}
			}
			prHeadCache[prHeadKey(p.repo, p.head)] = prHeadEntry{pr: raw, at: now}
		}
		prHeadMu.Unlock()
	}
	if failed {
		prHeadMu.Lock()
		prNextRetry = time.Now().Add(prErrCooldown)
		prHeadMu.Unlock()
	}
}

const prBatch = 20

const prNodeFields = `        number title state url isDraft mergeable mergeStateStatus
        headRefName baseRefName additions deletions changedFiles createdAt updatedAt
        reviewDecision
        author { login }
        latestReviews(first: 30) { nodes { state submittedAt author { login } } }
        commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
          __typename
          ... on CheckRun { name status conclusion detailsUrl startedAt checkSuite { workflowRun { workflow { name } } } }
          ... on StatusContext { context state targetUrl }
        } } } } } }
`

func ownerRepo(wt string) (owner, name string, ok bool) {
	out, err := exec.Command("git", "-C", wt, "remote", "get-url", "origin").Output()
	if err != nil {
		return "", "", false
	}
	url := strings.TrimSpace(string(out))
	url = strings.TrimSuffix(url, ".git")

	var path string
	if i := strings.Index(url, "://"); i >= 0 {
		rest := url[i+3:]
		if s := strings.IndexByte(rest, '/'); s >= 0 {
			path = rest[s+1:]
		}
	} else if i := strings.LastIndex(url, ":"); i >= 0 {
		path = url[i+1:]
	}

	parts := strings.Split(strings.Trim(path, "/"), "/")
	if len(parts) >= 2 {
		owner, name = parts[len(parts)-2], parts[len(parts)-1]
		if owner != "" && name != "" {
			return owner, name, true
		}
	}
	return "", "", false
}

type gqlPR struct {
	Number           int
	Title            string
	State            string
	URL              string
	IsDraft          bool
	Mergeable        string
	MergeStateStatus string
	HeadRefName      string
	BaseRefName      string
	Additions        int
	Deletions        int
	ChangedFiles     int
	CreatedAt        string
	UpdatedAt        string
	ReviewDecision   string
	Author           *struct {
		Login string `json:"login"`
	}
	LatestReviews struct {
		Nodes []struct {
			State       string `json:"state"`
			SubmittedAt string `json:"submittedAt"`
			Author      *struct {
				Login string `json:"login"`
			} `json:"author"`
		} `json:"nodes"`
	}
	Commits struct {
		Nodes []struct {
			Commit struct {
				StatusCheckRollup *struct {
					Contexts struct {
						Nodes []struct {
							Typename   string `json:"__typename"`
							Name       string `json:"name"`
							Status     string `json:"status"`
							Conclusion string `json:"conclusion"`
							DetailsURL string `json:"detailsUrl"`
							StartedAt  string `json:"startedAt"`
							CheckSuite *struct {
								WorkflowRun *struct {
									Workflow struct {
										Name string `json:"name"`
									} `json:"workflow"`
								} `json:"workflowRun"`
							} `json:"checkSuite"`
							Context   string `json:"context"`
							State     string `json:"state"`
							TargetURL string `json:"targetUrl"`
						} `json:"nodes"`
					} `json:"contexts"`
				} `json:"statusCheckRollup"`
			} `json:"commit"`
		} `json:"nodes"`
	}
	// Detail-only (empty on the board query).
	Body   string `json:"body"`
	Labels struct {
		Nodes []struct{ Name, Color string } `json:"nodes"`
	} `json:"labels"`
	ReviewRequests struct {
		Nodes []struct {
			RequestedReviewer *struct{ Login, Name, Slug string } `json:"requestedReviewer"`
		} `json:"nodes"`
	} `json:"reviewRequests"`
	Comments struct {
		Nodes []struct {
			Author    *struct{ Login string } `json:"author"`
			Body      string                  `json:"body"`
			CreatedAt string                  `json:"createdAt"`
		} `json:"nodes"`
	} `json:"comments"`
}

type ghActor struct {
	Login string `json:"login"`
}
type ghReview struct {
	Author      *ghActor `json:"author,omitempty"`
	State       string   `json:"state"`
	SubmittedAt string   `json:"submittedAt,omitempty"`
}
type ghCheck struct {
	Name         string `json:"name,omitempty"`
	Status       string `json:"status,omitempty"`
	Conclusion   string `json:"conclusion,omitempty"`
	DetailsURL   string `json:"detailsUrl,omitempty"`
	StartedAt    string `json:"startedAt,omitempty"`
	WorkflowName string `json:"workflowName,omitempty"`
	Result       string `json:"result"`
}
type ghLabel struct {
	Name  string `json:"name"`
	Color string `json:"color"`
}
type ghReviewReq struct {
	Login string `json:"login,omitempty"`
	Name  string `json:"name,omitempty"`
	Slug  string `json:"slug,omitempty"`
}
type ghComment struct {
	Author    *ghActor `json:"author,omitempty"`
	Body      string   `json:"body"`
	CreatedAt string   `json:"createdAt,omitempty"`
}
type ghPR struct {
	Number            int           `json:"number"`
	Title             string        `json:"title"`
	State             string        `json:"state"`
	URL               string        `json:"url"`
	IsDraft           bool          `json:"isDraft"`
	Mergeable         string        `json:"mergeable"`
	MergeStateStatus  string        `json:"mergeStateStatus"`
	HeadRefName       string        `json:"headRefName"`
	BaseRefName       string        `json:"baseRefName"`
	Author            *ghActor      `json:"author"`
	Reviews           []ghReview    `json:"reviews"`
	ReviewDecision    string        `json:"reviewDecision"`
	StatusCheckRollup []ghCheck     `json:"statusCheckRollup"`
	Additions         int           `json:"additions"`
	Deletions         int           `json:"deletions"`
	ChangedFiles      int           `json:"changedFiles"`
	CreatedAt         string        `json:"createdAt"`
	UpdatedAt         string        `json:"updatedAt"`
	Body              string        `json:"body,omitempty"`
	Labels            []ghLabel     `json:"labels,omitempty"`
	ReviewRequests    []ghReviewReq `json:"reviewRequests,omitempty"`
	Comments          []ghComment   `json:"comments,omitempty"`

	// Semantic tokens the core derives (ADR 0001) so the UI only maps token -> colour.
	// Always emitted (no omitempty): the Swift decoder needs the keys present.
	Checks    string       `json:"checks"`
	Review    string       `json:"review"`
	Conflict  bool         `json:"conflict"`
	Reviewers []ghReviewer `json:"reviewers"`
}

func (p gqlPR) toGH() ghPR {
	out := ghPR{
		Number:           p.Number,
		Title:            p.Title,
		State:            p.State,
		URL:              p.URL,
		IsDraft:          p.IsDraft,
		Mergeable:        p.Mergeable,
		MergeStateStatus: p.MergeStateStatus,
		HeadRefName:      p.HeadRefName,
		BaseRefName:      p.BaseRefName,
		ReviewDecision:   p.ReviewDecision,
		Additions:        p.Additions,
		Deletions:        p.Deletions,
		ChangedFiles:     p.ChangedFiles,
		CreatedAt:        p.CreatedAt,
		UpdatedAt:        p.UpdatedAt,
	}
	if p.Author != nil {
		out.Author = &ghActor{Login: p.Author.Login}
	}
	for _, rv := range p.LatestReviews.Nodes {
		r := ghReview{State: rv.State, SubmittedAt: rv.SubmittedAt}
		if rv.Author != nil {
			r.Author = &ghActor{Login: rv.Author.Login}
		}
		out.Reviews = append(out.Reviews, r)
	}
	out.Body = p.Body
	for _, l := range p.Labels.Nodes {
		out.Labels = append(out.Labels, ghLabel{Name: l.Name, Color: l.Color})
	}
	for _, rr := range p.ReviewRequests.Nodes {
		if rr.RequestedReviewer != nil {
			out.ReviewRequests = append(out.ReviewRequests, ghReviewReq{Login: rr.RequestedReviewer.Login, Name: rr.RequestedReviewer.Name, Slug: rr.RequestedReviewer.Slug})
		}
	}
	for _, c := range p.Comments.Nodes {
		cm := ghComment{Body: c.Body, CreatedAt: c.CreatedAt}
		if c.Author != nil {
			cm.Author = &ghActor{Login: c.Author.Login}
		}
		out.Comments = append(out.Comments, cm)
	}
	if len(p.Commits.Nodes) > 0 {
		roll := p.Commits.Nodes[0].Commit.StatusCheckRollup
		if roll != nil {
			for _, c := range roll.Contexts.Nodes {
				switch c.Typename {
				case "CheckRun":
					wf := ""
					if c.CheckSuite != nil && c.CheckSuite.WorkflowRun != nil {
						wf = c.CheckSuite.WorkflowRun.Workflow.Name
					}
					out.StatusCheckRollup = append(out.StatusCheckRollup, ghCheck{
						Name: c.Name, Status: c.Status, Conclusion: c.Conclusion,
						DetailsURL: c.DetailsURL, StartedAt: c.StartedAt, WorkflowName: wf,
					})
				case "StatusContext":
					chk := ghCheck{Name: c.Context, DetailsURL: c.TargetURL}
					switch c.State {
					case "SUCCESS":
						chk.Status, chk.Conclusion = "COMPLETED", "SUCCESS"
					case "FAILURE", "ERROR":
						chk.Status, chk.Conclusion = "COMPLETED", "FAILURE"
					default:
						chk.Status = "PENDING"
					}
					out.StatusCheckRollup = append(out.StatusCheckRollup, chk)
				}
			}
		}
	}
	out.classify()
	return out
}
