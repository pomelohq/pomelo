package core

import "testing"

func asMap(v any) map[string]any {
	m, _ := v.(map[string]any)
	return m
}

func TestQueryUnknownDomain(t *testing.T) {
	s := &Server{}
	if asMap(s.Query("nope", nil))["error"] == nil {
		t.Fatalf("unknown query domain should return an error, got %v", s.Query("nope", nil))
	}
}

func TestCommandUnknownDomain(t *testing.T) {
	s := &Server{}
	r := asMap(s.Command("nope", "do", nil))
	if r["ok"] != false || r["error"] == nil {
		t.Fatalf("unknown command domain should be {ok:false,error}, got %v", r)
	}
}

func TestVerbsNilServerSafe(t *testing.T) {
	var s *Server
	if asMap(s.Query("workspaces", nil))["error"] == nil {
		t.Fatalf("nil server Query should return an error")
	}
	if asMap(s.Command("service", "stop", nil))["error"] == nil {
		t.Fatalf("nil server Command should return an error")
	}
}
