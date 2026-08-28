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

func buildTimeline(detailBytes, commentsBytes []byte) []byte {
	empty := []byte(`{"items":[]}`)

	var detail struct {
		PR *struct {
			Body      string          `json:"body"`
			Author    *ghActor        `json:"author"`
			ReviewLog []ghReviewEntry `json:"reviewLog"`
			Comments  []ghComment     `json:"comments"`
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

	// No omitempty on state/inline: Swift's synthesized Decodable requires every
	// key even when the property has a default, so an absent key breaks the decode.
	type item struct {
		ID     string            `json:"id"`
		Kind   string            `json:"kind"`
		Author string            `json:"author,omitempty"`
		Avatar string            `json:"avatar,omitempty"`
		Body   string            `json:"body"`
		At     string            `json:"at"`
		State  string            `json:"state"`
		Inline []json.RawMessage `json:"inline"`
	}
	var out []item

	if strings.TrimSpace(pr.Body) != "" {
		out = append(out, item{ID: "body", Kind: "description", Author: actorLogin(pr.Author), Avatar: actorAvatar(pr.Author), Body: pr.Body})
	}

	known := map[int64]bool{}
	for _, r := range pr.ReviewLog {
		known[r.ReviewID] = true
	}
	byReview := map[int64][]json.RawMessage{}
	var loose []json.RawMessage
	for _, raw := range cs.Comments {
		var meta struct {
			Body     string `json:"body"`
			ReviewID *int64 `json:"reviewId"`
		}
		_ = json.Unmarshal(raw, &meta)
		if meta.Body == "" {
			continue
		}
		if meta.ReviewID != nil && known[*meta.ReviewID] {
			byReview[*meta.ReviewID] = append(byReview[*meta.ReviewID], raw)
		} else {
			loose = append(loose, raw)
		}
	}

	for _, r := range pr.ReviewLog {
		inline := byReview[r.ReviewID]
		state := strings.ToUpper(r.State)
		if r.Body == "" && len(inline) == 0 && state == "COMMENTED" {
			continue
		}
		it := item{ID: "review-" + strconv.FormatInt(r.ReviewID, 10), Kind: "review", Author: actorLogin(r.Author), Avatar: actorAvatar(r.Author),
			Body: r.Body, At: r.SubmittedAt, State: state, Inline: inline}
		out = append(out, it)
	}

	for _, raw := range loose {
		var c struct {
			User      string `json:"user"`
			AvatarURL string `json:"avatarUrl"`
			Body      string `json:"body"`
			CreatedAt string `json:"createdAt"`
			Path      string `json:"path"`
			Line      *int   `json:"line"`
		}
		_ = json.Unmarshal(raw, &c)
		id := "inline-" + c.User + c.Path
		if c.Line != nil {
			id += strconv.Itoa(*c.Line)
		}
		out = append(out, item{ID: id, Kind: "inline", Author: c.User, Avatar: c.AvatarURL, Body: c.Body, At: c.CreatedAt, Inline: []json.RawMessage{raw}})
	}

	for i, c := range pr.Comments {
		if c.Body == "" {
			continue
		}
		out = append(out, item{ID: "comment-" + strconv.Itoa(i) + actorLogin(c.Author), Kind: "comment",
			Author: actorLogin(c.Author), Avatar: actorAvatar(c.Author), Body: c.Body, At: c.CreatedAt})
	}

	for i := range out {
		if out[i].Inline == nil {
			out[i].Inline = []json.RawMessage{}
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
