package forge

import (
	"encoding/json"
	"testing"
)

func TestChecksToken(t *testing.T) {
	cases := []struct {
		name string
		in   []ghCheck
		want string
	}{
		{"empty", nil, "none"},
		{"any fail wins", []ghCheck{{Conclusion: "SUCCESS"}, {Conclusion: "FAILURE"}}, "fail"},
		{"pending over pass", []ghCheck{{Conclusion: "SUCCESS"}, {Status: "IN_PROGRESS"}}, "pending"},
		{"all pass", []ghCheck{{Conclusion: "SUCCESS"}, {Conclusion: "SUCCESS"}}, "pass"},
		{"neutral only", []ghCheck{{Conclusion: "NEUTRAL"}}, "none"},
	}
	for _, c := range cases {
		if got := checksToken(c.in); got != c.want {
			t.Errorf("%s: got %q want %q", c.name, got, c.want)
		}
	}
}

func TestReviewToken(t *testing.T) {
	if got := reviewToken("APPROVED", nil); got != "approved" {
		t.Errorf("decision approved: %q", got)
	}
	if got := reviewToken("CHANGES_REQUESTED", nil); got != "changes" {
		t.Errorf("decision changes: %q", got)
	}
	if got := reviewToken("REVIEW_REQUIRED", nil); got != "review" {
		t.Errorf("decision review: %q", got)
	}
	// No decision -> fall back to review states; changes wins over approved.
	revs := []ghReview{{State: "APPROVED"}, {State: "CHANGES_REQUESTED"}}
	if got := reviewToken("", revs); got != "changes" {
		t.Errorf("fallback changes: %q", got)
	}
	if got := reviewToken("", []ghReview{{State: "APPROVED"}}); got != "approved" {
		t.Errorf("fallback approved: %q", got)
	}
	if got := reviewToken("", []ghReview{{State: "COMMENTED"}}); got != "review" {
		t.Errorf("fallback review: %q", got)
	}
	if got := reviewToken("", nil); got != "none" {
		t.Errorf("none: %q", got)
	}
}

func TestConflictFlag(t *testing.T) {
	if !conflictFlag("conflicting") || !conflictFlag("CONFLICTING") {
		t.Error("conflicting should be true")
	}
	if conflictFlag("MERGEABLE") {
		t.Error("mergeable should be false")
	}
}

func TestReviewersDedupeAndPending(t *testing.T) {
	revs := []ghReview{
		{Author: &ghActor{Login: "bob"}, State: "COMMENTED"},
		{Author: &ghActor{Login: "bob"}, State: "APPROVED"},
	}
	reqs := []ghReviewReq{{Login: "carol"}, {Login: "bob"}}
	out := reviewersList(revs, reqs)
	if len(out) != 2 {
		t.Fatalf("want 2 reviewers, got %d: %+v", len(out), out)
	}
	if out[0].Name != "bob" || out[0].State != "approved" {
		t.Errorf("bob latest state should win (approved): %+v", out[0])
	}
	if out[1].Name != "carol" || out[1].State != "pending" {
		t.Errorf("carol should be pending (not already reviewed): %+v", out[1])
	}
}

func TestSeverity(t *testing.T) {
	blob := func(state, checks, review string, conflict bool) json.RawMessage {
		b, _ := json.Marshal(map[string]any{"state": state, "checks": checks, "review": review, "conflict": conflict})
		return b
	}
	cases := []struct {
		name string
		in   []json.RawMessage
		want string
	}{
		{"conflict is danger", []json.RawMessage{blob("OPEN", "pass", "approved", true)}, "danger"},
		{"failing checks is danger", []json.RawMessage{blob("OPEN", "fail", "none", false)}, "danger"},
		{"changes requested is danger", []json.RawMessage{blob("OPEN", "pass", "changes", false)}, "danger"},
		{"pending is warn", []json.RawMessage{blob("OPEN", "pending", "none", false)}, "warn"},
		{"clean is ok", []json.RawMessage{blob("OPEN", "pass", "approved", false)}, "ok"},
		{"merged only", []json.RawMessage{blob("MERGED", "", "", false)}, "merged"},
		{"danger beats merged", []json.RawMessage{blob("MERGED", "", "", false), blob("OPEN", "fail", "none", false)}, "danger"},
	}
	for _, c := range cases {
		if got := prSeverity(c.in); got != c.want {
			t.Errorf("%s: got %q want %q", c.name, got, c.want)
		}
	}
}

func TestClassifySetsTokens(t *testing.T) {
	p := ghPR{
		State:          "OPEN",
		Mergeable:      "CONFLICTING",
		ReviewDecision: "CHANGES_REQUESTED",
		StatusCheckRollup: []ghCheck{
			{Name: "build", Conclusion: "SUCCESS"},
			{Name: "test", Status: "IN_PROGRESS"},
		},
	}
	p.classify()
	if p.Checks != "pending" {
		t.Errorf("checks: %q", p.Checks)
	}
	if p.Review != "changes" {
		t.Errorf("review: %q", p.Review)
	}
	if !p.Conflict {
		t.Error("conflict should be true")
	}
	if p.StatusCheckRollup[0].Result != "pass" || p.StatusCheckRollup[1].Result != "pending" {
		t.Errorf("per-check result: %q %q", p.StatusCheckRollup[0].Result, p.StatusCheckRollup[1].Result)
	}
}
