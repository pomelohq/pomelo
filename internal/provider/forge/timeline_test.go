package forge

import (
	"encoding/json"
	"testing"
)

type tlThread struct {
	Resolved bool `json:"resolved"`
	Comments []struct {
		User string `json:"user"`
		Body string `json:"body"`
	} `json:"comments"`
}

type tlDecoded struct {
	Items []struct {
		ID       string     `json:"id"`
		Kind     string     `json:"kind"`
		Body     string     `json:"body"`
		State    string     `json:"state"`
		Resolved bool       `json:"resolved"`
		Threads  []tlThread `json:"threads"`
	} `json:"items"`
}

func TestBuildTimeline(t *testing.T) {
	// Post-toGH cache shape: reviewThreads carry `resolved`, a `reviewId` linking
	// them to a review, and `user`-shaped comments.
	detail := []byte(`{"pr":{
		"body":"Adds group showing setup.",
		"author":{"login":"alice","avatarUrl":"a.png"},
		"reviewLog":[
			{"reviewId":1,"state":"APPROVED","body":"lgtm","submittedAt":"2026-08-20T10:00:00Z","author":{"login":"bob"}},
			{"reviewId":2,"state":"COMMENTED","body":"","submittedAt":"2026-08-21T10:00:00Z","author":{"login":"carol"}}
		],
		"comments":[{"author":{"login":"dave"},"body":"nice","createdAt":"2026-08-22T10:00:00Z"}],
		"reviewThreads":[
			{"resolved":true,"reviewId":1,"path":"a.go","line":10,"at":"2026-08-20T10:00:00Z","comments":[
				{"user":"bob","body":"fix this","path":"a.go","line":10,"createdAt":"2026-08-20T10:00:00Z"},
				{"user":"alice","body":"done","path":"a.go","line":10,"createdAt":"2026-08-20T10:30:00Z"}
			]},
			{"resolved":false,"reviewId":2,"path":"b.go","line":3,"at":"2026-08-21T10:00:00Z","comments":[
				{"user":"carol","body":"nit","path":"b.go","line":3,"createdAt":"2026-08-21T10:00:00Z"}
			]}
		]
	}}`)

	var got tlDecoded
	if err := json.Unmarshal(buildTimeline(detail, nil), &got); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if len(got.Items) == 0 {
		t.Fatalf("no items")
	}
	if got.Items[0].Kind != "description" {
		t.Fatalf("first item not description: %+v", got.Items[0])
	}
	sawComment := false
	var review1Threads, review2Threads int
	var review1Resolved bool
	var review1RootComments int
	for _, it := range got.Items {
		switch it.ID {
		case "review-1":
			review1Threads = len(it.Threads)
			if len(it.Threads) > 0 {
				review1Resolved = it.Threads[0].Resolved
				review1RootComments = len(it.Threads[0].Comments)
			}
		case "review-2":
			review2Threads = len(it.Threads)
		}
		if it.Kind == "comment" {
			sawComment = true
		}
	}
	if !sawComment {
		t.Fatalf("missing issue comment")
	}
	if review1Threads != 1 || !review1Resolved || review1RootComments != 2 {
		t.Fatalf("review-1 should nest 1 resolved thread with 2 comments, got threads=%d resolved=%v comments=%d", review1Threads, review1Resolved, review1RootComments)
	}
	// review-2 was an empty COMMENTED review but has a thread, so it must be kept.
	if review2Threads != 1 {
		t.Fatalf("review-2 should be kept and nest 1 thread, got %d", review2Threads)
	}

	var rawItems struct {
		Items []map[string]json.RawMessage `json:"items"`
	}
	_ = json.Unmarshal(buildTimeline(detail, nil), &rawItems)
	for i, it := range rawItems.Items {
		for _, k := range []string{"state", "threads", "resolved"} {
			if _, ok := it[k]; !ok {
				t.Fatalf("item %d missing %q key", i, k)
			}
		}
	}
}

// A thread whose review isn't in the reviewLog stands alone as its own item.
func TestBuildTimelineStandaloneThread(t *testing.T) {
	detail := []byte(`{"pr":{"body":"x","author":{"login":"alice"},"reviewLog":[],"comments":[],
		"reviewThreads":[{"resolved":false,"reviewId":999,"path":"a.go","line":1,"at":"2026-08-20T10:00:00Z","comments":[
			{"user":"bob","body":"hey","path":"a.go","line":1,"createdAt":"2026-08-20T10:00:00Z"}
		]}]}}`)
	var got tlDecoded
	if err := json.Unmarshal(buildTimeline(detail, nil), &got); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	inline := 0
	for _, it := range got.Items {
		if it.Kind == "inline" && len(it.Threads) == 1 {
			inline++
		}
	}
	if inline != 1 {
		t.Fatalf("standalone thread should be 1 inline item, got %d", inline)
	}
}

// A cache blob from before reviewThreads existed: inline comments arrive via the
// separate comments fetch and each becomes a one-comment thread.
func TestBuildTimelineFallback(t *testing.T) {
	detail := []byte(`{"pr":{"body":"x","author":{"login":"alice"},"reviewLog":[],"comments":[]}}`)
	comments := []byte(`{"comments":[
		{"user":"carol","body":"fix this","path":"a.go","line":10,"createdAt":"2026-08-21T09:00:00Z"},
		{"user":"erin","body":"loose note","path":"b.go","line":3,"createdAt":"2026-08-23T10:00:00Z"}
	]}`)
	var got tlDecoded
	if err := json.Unmarshal(buildTimeline(detail, comments), &got); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	inline := 0
	for _, it := range got.Items {
		if it.Kind == "inline" {
			inline++
		}
	}
	if inline != 2 {
		t.Fatalf("fallback should emit 2 inline items, got %d", inline)
	}
}

func TestBuildTimelineEmpty(t *testing.T) {
	if string(buildTimeline([]byte(`{"pr":null}`), []byte(`{"comments":[]}`))) != `{"items":[]}` {
		t.Fatalf("expected empty items")
	}
}
