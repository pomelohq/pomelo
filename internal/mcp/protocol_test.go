package mcp

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestServeProtocol(t *testing.T) {
	tools := []Tool{
		{
			Name: "echo", Description: "echo back",
			Schema: map[string]any{"type": "object", "properties": map[string]any{"msg": map[string]any{"type": "string"}}},
			Run:    func(a map[string]any) (string, error) { return "you said: " + str(a, "msg"), nil },
		},
		{
			Name: "acme", Description: "always fails",
			Run: func(map[string]any) (string, error) { return "", errFail },
		},
	}
	in := strings.Join([]string{
		`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}`,
		`{"jsonrpc":"2.0","method":"notifications/initialized"}`,
		`{"jsonrpc":"2.0","id":2,"method":"tools/list"}`,
		`{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"msg":"hi"}}}`,
		`{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"acme","arguments":{}}}`,
		`{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}`,
	}, "\n") + "\n"

	var out strings.Builder
	if err := Serve(strings.NewReader(in), &out, "pomelo", "test", tools); err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(out.String()), "\n")
	if len(lines) != 5 {
		t.Fatalf("expected 5 responses, got %d: %q", len(lines), out.String())
	}

	var initR struct {
		Result struct {
			ProtocolVersion string         `json:"protocolVersion"`
			Capabilities    map[string]any `json:"capabilities"`
		} `json:"result"`
	}
	mustJSON(t, lines[0], &initR)
	if initR.Result.ProtocolVersion != "2025-06-18" {
		t.Errorf("protocol version = %q", initR.Result.ProtocolVersion)
	}
	if _, ok := initR.Result.Capabilities["tools"]; !ok {
		t.Error("missing tools capability")
	}

	var listR struct {
		Result struct {
			Tools []struct{ Name string } `json:"tools"`
		} `json:"result"`
	}
	mustJSON(t, lines[1], &listR)
	if len(listR.Result.Tools) != 2 {
		t.Errorf("tools/list = %d tools", len(listR.Result.Tools))
	}

	if !strings.Contains(lines[2], "you said: hi") {
		t.Errorf("echo result: %s", lines[2])
	}
	if !strings.Contains(lines[3], `"isError":true`) || !strings.Contains(lines[3], "fail") {
		t.Errorf("error result not surfaced: %s", lines[3])
	}
	if !strings.Contains(lines[4], "unknown tool") {
		t.Errorf("unknown tool not surfaced: %s", lines[4])
	}
}

var errFail = errTest("fail")

type errTest string

func (e errTest) Error() string { return string(e) }

func mustJSON(t *testing.T, line string, v any) {
	t.Helper()
	if err := json.Unmarshal([]byte(line), v); err != nil {
		t.Fatalf("bad json %q: %v", line, err)
	}
}

func TestToolListAnnotations(t *testing.T) {
	tools := []Tool{
		{Name: "peek", ReadOnly: true, Run: func(map[string]any) (string, error) { return "", nil }},
		{Name: "nuke", Destructive: true, Run: func(map[string]any) (string, error) { return "", nil }},
		{Name: "poke", Run: func(map[string]any) (string, error) { return "", nil }},
	}
	got := map[string]map[string]any{}
	for _, e := range toolList(tools) {
		got[e["name"].(string)] = e["annotations"].(map[string]any)
	}
	if a := got["peek"]; a["readOnlyHint"] != true || a["destructiveHint"] != false || a["idempotentHint"] != true {
		t.Errorf("peek annotations wrong: %v", a)
	}
	if a := got["nuke"]; a["readOnlyHint"] != false || a["destructiveHint"] != true {
		t.Errorf("nuke annotations wrong: %v", a)
	}
	if a := got["poke"]; a["readOnlyHint"] != false || a["destructiveHint"] != false {
		t.Errorf("poke annotations wrong: %v", a)
	}
}

func TestResultCap(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	big := strings.Repeat("line of output\n", 5000)
	tools := []Tool{{Name: "dump", MaxResultChars: 8000,
		Run: func(map[string]any) (string, error) { return big, nil }}}
	res, rerr := callTool(map[string]Tool{"dump": tools[0]}, json.RawMessage(`{"name":"dump","arguments":{}}`))
	if rerr != nil {
		t.Fatal(rerr)
	}
	text := res.(map[string]any)["content"].([]map[string]any)[0]["text"].(string)
	if len(text) >= len(big) {
		t.Fatalf("result not capped: %d >= %d", len(text), len(big))
	}
	if !strings.Contains(text, "truncated") || !strings.Contains(text, "mcp-out") {
		t.Fatalf("missing truncation notice/pointer: %q", text[len(text)-200:])
	}
}
