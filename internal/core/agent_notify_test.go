package core

import "testing"

func TestAgentNotification(t *testing.T) {
	cases := []struct {
		from, to   string
		wantEvent  string
		wantNotify bool
	}{
		{"", "awaiting_input", "needs_input", true},
		{"thinking", "awaiting_input", "needs_input", true},
		{"awaiting_input", "awaiting_input", "", false},
		{"idle", "compacting", "compacting", true},
		{"thinking", "idle", "finished", true},
		{"tool_use", "stopped", "finished", true},
		{"", "thinking", "working", true},
		{"idle", "tool_use", "working", true},
		{"idle", "idle", "", false},
		{"thinking", "tool_use", "", false},
	}
	for _, c := range cases {
		title, event, ok := agentNotification(c.from, c.to)
		if ok != c.wantNotify || event != c.wantEvent {
			t.Errorf("(%q->%q): got (%q,%q,%v), want event %q notify %v", c.from, c.to, title, event, ok, c.wantEvent, c.wantNotify)
		}
		if ok && title == "" {
			t.Errorf("(%q->%q): notify but empty title", c.from, c.to)
		}
	}
}

func TestAgentNotificationsDiffsAndClassifies(t *testing.T) {
	prev := map[string]string{"a": "thinking", "b": "idle"}
	next := map[string]string{"a": "idle", "b": "idle", "c": "thinking"}
	notes := AgentNotifications(prev, next)
	got := map[string]string{}
	for _, n := range notes {
		got[n["ws"].(string)] = n["event"].(string)
	}
	if got["a"] != "finished" {
		t.Errorf("a: want finished, got %q", got["a"])
	}
	if got["c"] != "working" {
		t.Errorf("c: want working, got %q", got["c"])
	}
	if _, ok := got["b"]; ok {
		t.Error("b unchanged must not notify")
	}
}
