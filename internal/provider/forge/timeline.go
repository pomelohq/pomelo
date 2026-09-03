package forge

import (
	"encoding/json"
	"sort"
	"strconv"
	"strings"
)

// Timeline builds the PR conversation timeline (ADR 0001: grouping reviews with
// their inline comments is domain logic, so it lives in the core; the UI renders).
func (s *Feature) Timeline(branch, repo string, isMain bool) []byte {
	return buildTimeline(s.PRDetail(branch, repo, isMain), s.PRComments(branch, repo, isMain))
}

// No omitempty on state/threads/resolved: Swift's synthesized Decodable requires
// every key even when the property has a default, so an absent key breaks decode.
type tlItem struct {
	ID       string            `json:"id"`
	Kind     string            `json:"kind"`
	Author   string            `json:"author,omitempty"`
	Avatar   string            `json:"avatar,omitempty"`
	Body     string            `json:"body"`
	At       string            `json:"at"`
	State    string            `json:"state"`
	Resolved bool              `json:"resolved"`
	Threads  []json.RawMessage `json:"threads"`
}

// Fallback for a pre-reviewThreads cache blob: wrap each inline comment as a
// one-comment thread so the UI renders it the same way.
func buildInlineFromComments(comments []json.RawMessage, out *[]tlItem) {
	for _, raw := range comments {
		var c struct {
			User      string `json:"user"`
			AvatarURL string `json:"avatarUrl"`
			Body      string `json:"body"`
			CreatedAt string `json:"createdAt"`
			Path      string `json:"path"`
			Line      *int   `json:"line"`
		}
		_ = json.Unmarshal(raw, &c)
		if c.Body == "" {
			continue
		}
		id := "inline-" + c.User + c.Path
		if c.Line != nil {
			id += strconv.Itoa(*c.Line)
		}
		th, _ := json.Marshal(map[string]any{"resolved": false, "path": c.Path, "line": c.Line,
			"at": c.CreatedAt, "comments": []json.RawMessage{raw}})
		*out = append(*out, tlItem{ID: id, Kind: "inline", Author: c.User, Avatar: c.AvatarURL,
			At: c.CreatedAt, Threads: []json.RawMessage{th}})
	}
}

func buildTimeline(detailBytes, commentsBytes []byte) []byte {
	empty := []byte(`{"items":[]}`)

	var detail struct {
		PR *struct {
			Body          string           `json:"body"`
			Author        *ghActor         `json:"author"`
			ReviewLog     []ghReviewEntry  `json:"reviewLog"`
			Comments      []ghComment      `json:"comments"`
			ReviewThreads []ghReviewThread `json:"reviewThreads"`
		} `json:"pr"`
	}
	if json.Unmarshal(detailBytes, &detail) != nil || detail.PR == nil {
		return empty
	}
	pr := detail.PR

	var cs struct {
		Comments []json.RawMessage `json:"comments"`
	}
	_ = json.Unmarshal(commentsBytes, &cs)

	var out []tlItem

	if strings.TrimSpace(pr.Body) != "" {
		out = append(out, tlItem{ID: "body", Kind: "description", Author: actorLogin(pr.Author), Avatar: actorAvatar(pr.Author), Body: pr.Body})
	}

	// GitHub nests a review's inline threads under the "reviewed" event, so group
	// each thread by the review that created its root comment.
	known := map[int64]bool{}
	for _, r := range pr.ReviewLog {
		known[r.ReviewID] = true
	}
	byReview := map[int64][]json.RawMessage{}
	var standalone []ghReviewThread
	for _, t := range pr.ReviewThreads {
		if len(t.Comments) == 0 {
			continue
		}
		b, _ := json.Marshal(t)
		if t.ReviewID != 0 && known[t.ReviewID] {
			byReview[t.ReviewID] = append(byReview[t.ReviewID], b)
		} else {
			standalone = append(standalone, t)
		}
	}

	for _, r := range pr.ReviewLog {
		state := strings.ToUpper(r.State)
		ths := byReview[r.ReviewID]
		if r.Body == "" && len(ths) == 0 && state == "COMMENTED" {
			continue
		}
		out = append(out, tlItem{ID: "review-" + strconv.FormatInt(r.ReviewID, 10), Kind: "review",
			Author: actorLogin(r.Author), Avatar: actorAvatar(r.Author), Body: r.Body, At: r.SubmittedAt, State: state, Threads: ths})
	}

	if len(pr.ReviewThreads) > 0 {
		for i, t := range standalone {
			b, _ := json.Marshal(t)
			id := "thread-" + strconv.Itoa(i) + t.Path
			if t.Line != nil {
				id += strconv.Itoa(*t.Line)
			}
			out = append(out, tlItem{ID: id, Kind: "inline", Author: t.Comments[0].User, Avatar: t.Comments[0].AvatarURL,
				At: t.At, Resolved: t.Resolved, Threads: []json.RawMessage{b}})
		}
	} else {
		buildInlineFromComments(cs.Comments, &out)
	}

	for i, c := range pr.Comments {
		if c.Body == "" {
			continue
		}
		out = append(out, tlItem{ID: "comment-" + strconv.Itoa(i) + actorLogin(c.Author), Kind: "comment",
			Author: actorLogin(c.Author), Avatar: actorAvatar(c.Author), Body: c.Body, At: c.CreatedAt})
	}

	for i := range out {
		if out[i].Threads == nil {
			out[i].Threads = []json.RawMessage{}
		}
	}

	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Kind == "description" {
			return true
		}
		if out[j].Kind == "description" {
			return false
		}
		return out[i].At < out[j].At
	})

	b, err := json.Marshal(map[string]any{"items": out})
	if err != nil {
		return empty
	}
	return b
}

func actorLogin(a *ghActor) string {
	if a != nil {
		return a.Login
	}
	return ""
}
func actorAvatar(a *ghActor) string {
	if a != nil {
		return a.AvatarURL
	}
	return ""
}
