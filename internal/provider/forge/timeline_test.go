package forge

import (
	"encoding/json"
	"testing"
)

func TestBuildTimeline(t *testing.T) {
	detail := []byte(`{"pr":{
		"body":"Adds group showing setup.",
		"author":{"login":"alice","avatarUrl":"a.png"},
		"reviewLog":[
			{"reviewId":1,"state":"APPROVED","body":"lgtm","submittedAt":"2026-08-20T10:00:00Z","author":{"login":"bob"}},
			{"reviewId":2,"state":"COMMENTED","body":"","submittedAt":"2026-08-21T10:00:00Z","author":{"login":"carol"}}
		],
		"comments":[{"author":{"login":"dave"},"body":"nice","createdAt":"2026-08-22T10:00:00Z"}]
	}}`)
	comments := []byte(`{"comments":[
		{"user":"carol","body":"fix this","path":"a.go","line":10,"reviewId":2,"createdAt":"2026-08-21T09:00:00Z"},
		{"user":"erin","body":"loose note","path":"b.go","line":3,"createdAt":"2026-08-23T10:00:00Z"}
	]}`)

	var got struct {
		Items []struct {
			ID     string            `json:"id"`
			Kind   string            `json:"kind"`
			Body   string            `json:"body"`
			State  string            `json:"state"`
			Inline []json.RawMessage `json:"inline"`
		} `json:"items"`
	}
	if err := json.Unmarshal(buildTimeline(detail, comments), &got); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(got.Items) == 0 {
		t.Fatalf("no items")
	}
	if got.Items[0].Kind != "description" || got.Items[0].Body != "Adds group showing setup." {
		t.Fatalf("first item not description: %+v", got.Items[0])
	}
	var review2Inline int
	sawApproved, sawComment, sawLoose := false, false, false
	for _, it := range got.Items {
		switch {
		case it.Kind == "review" && it.State == "APPROVED":
			sawApproved = true
		case it.Kind == "review" && it.ID == "review-2":
			review2Inline = len(it.Inline)
		case it.Kind == "comment":
			sawComment = true
		case it.Kind == "inline":
			sawLoose = true
		}
	}
	if !sawApproved || !sawComment || !sawLoose {
		t.Fatalf("missing kinds: approved=%v comment=%v loose=%v items=%+v", sawApproved, sawComment, sawLoose, got.Items)
	}
	if review2Inline != 1 {
		t.Fatalf("review-2 should carry 1 inline comment, got %d", review2Inline)
	}

	// Every item must carry state+inline keys even when empty: the Swift decoder
	// requires the keys present (a default value does not cover an absent key).
	var rawItems struct {
		Items []map[string]json.RawMessage `json:"items"`
	}
	_ = json.Unmarshal(buildTimeline(detail, comments), &rawItems)
	for i, it := range rawItems.Items {
		if _, ok := it["state"]; !ok {
			t.Fatalf("item %d missing state key", i)
		}
		if _, ok := it["inline"]; !ok {
			t.Fatalf("item %d missing inline key", i)
		}
	}
}

func TestBuildTimelineEmpty(t *testing.T) {
	if string(buildTimeline([]byte(`{"pr":null}`), []byte(`{"comments":[]}`))) != `{"items":[]}` {
		t.Fatalf("expected empty items")
	}
}
