package forge

import (
	"encoding/json"
	"sort"
	"strings"
)

// PR status classification (ADR 0001): the core turns raw GitHub state into the
// semantic tokens the UI renders, so the frontend only maps token -> colour/text.

type ghReviewer struct {
	Name  string `json:"name"`
	State string `json:"state"`
}

func checkResultToken(status, conclusion string) string {
	v := strings.ToUpper(conclusion)
	s := strings.ToUpper(status)
	switch v {
	case "FAILURE", "ERROR", "TIMED_OUT", "CANCELLED":
		return "fail"
	case "SUCCESS":
		return "pass"
	}
	if v == "PENDING" || s == "IN_PROGRESS" || s == "QUEUED" || s == "PENDING" {
		return "pending"
	}
	return "none"
}

func checksToken(checks []ghCheck) string {
	if len(checks) == 0 {
		return "none"
	}
	pending, pass := false, false
	for _, c := range checks {
		switch checkResultToken(c.Status, c.Conclusion) {
		case "fail":
			return "fail"
		case "pending":
			pending = true
		case "pass":
			pass = true
		}
	}
	if pending {
		return "pending"
	}
	if pass {
		return "pass"
	}
	return "none"
}

func reviewToken(reviewDecision string, reviews []ghReview) string {
	switch strings.ToUpper(reviewDecision) {
	case "APPROVED":
		return "approved"
	case "CHANGES_REQUESTED":
		return "changes"
	case "REVIEW_REQUIRED":
		return "review"
	}
	if len(reviews) == 0 {
		return "none"
	}
	approved := false
	for _, r := range reviews {
		switch strings.ToUpper(r.State) {
		case "CHANGES_REQUESTED":
			return "changes"
		case "APPROVED":
			approved = true
		}
	}
	if approved {
		return "approved"
	}
	return "review"
}

func conflictFlag(mergeable string) bool { return strings.ToUpper(mergeable) == "CONFLICTING" }

func reviewersList(reviews []ghReview, requests []ghReviewReq) []ghReviewer {
	latest := map[string]string{}
	for _, r := range reviews {
		if r.Author == nil || r.Author.Login == "" {
			continue
		}
		latest[r.Author.Login] = strings.ToUpper(r.State)
	}
	out := []ghReviewer{}
	for who, st := range latest {
		s := "commented"
		switch st {
		case "APPROVED":
			s = "approved"
		case "CHANGES_REQUESTED":
			s = "changes"
		}
		out = append(out, ghReviewer{Name: who, State: s})
	}
	for _, rr := range requests {
		who := rr.Login
		if who == "" {
			who = rr.Name
		}
		if who == "" {
			continue
		}
		if _, done := latest[who]; !done {
			out = append(out, ghReviewer{Name: who, State: "pending"})
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

func (p *ghPR) classify() {
	for i := range p.StatusCheckRollup {
		p.StatusCheckRollup[i].Result = checkResultToken(p.StatusCheckRollup[i].Status, p.StatusCheckRollup[i].Conclusion)
	}
	p.Checks = checksToken(p.StatusCheckRollup)
	p.Review = reviewToken(p.ReviewDecision, p.Reviews)
	p.Conflict = conflictFlag(p.Mergeable)
	p.Reviewers = reviewersList(p.Reviews, p.ReviewRequests)
}

// prSeverity aggregates a workspace's PR set into one token the sidebar renders.
func prSeverity(blobs []json.RawMessage) string {
	type tok struct {
		State    string `json:"state"`
		Checks   string `json:"checks"`
		Review   string `json:"review"`
		Conflict bool   `json:"conflict"`
	}
	worst, anyMerged := 0, false
	for _, b := range blobs {
		var t tok
		if json.Unmarshal(b, &t) != nil {
			continue
		}
		if t.State == "MERGED" {
			anyMerged = true
			continue
		}
		if t.Conflict || t.Checks == "fail" || t.Review == "changes" {
			worst = 2
		} else if worst < 1 && (t.Checks == "pending" || t.Review == "review") {
			worst = 1
		}
	}
	if worst == 2 {
		return "danger"
	}
	if anyMerged {
		return "merged"
	}
	if worst == 1 {
		return "warn"
	}
	return "ok"
}
