package core

import "testing"

func TestParseAckControl(t *testing.T) {
	if n, ok := parseAckControl([]byte(`{"__pom":"ack","n":4096}`)); !ok || n != 4096 {
		t.Fatalf("parseAckControl = %d, %v", n, ok)
	}
	for _, bad := range []string{`{"__pom":"resize","cols":80,"rows":24}`, "plain keystrokes", `{"__pom":"ack","n":0}`, `{"__pom":"ack","n":-5}`} {
		if _, ok := parseAckControl([]byte(bad)); ok {
			t.Errorf("parseAckControl accepted %q", bad)
		}
	}
}
