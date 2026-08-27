package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"encoding/json"

	"github.com/pomelohq/pomelo/internal/agent/claude"
	"github.com/pomelohq/pomelo/internal/agent/codeagent"
	"github.com/pomelohq/pomelo/internal/core"
)

//export PomClaudeUsage
func PomClaudeUsage() *C.char {
	return bindingJSON(claude.FetchUsage())
}

//export PomCodeAgents
func PomCodeAgents() *C.char {
	type agent struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}
	out := []agent{}
	for _, ca := range codeagent.Builtin() {
		out = append(out, agent{ID: ca.Cmd, Name: ca.Name})
	}
	return bindingJSON(out)
}

func server() *core.Server {
	mu.Lock()
	defer mu.Unlock()
	return srv
}

func bindingJSON(v any) *C.char {
	b, err := json.Marshal(v)
	if err != nil {
		return C.CString("{}")
	}
	return C.CString(string(b))
}

func bindingBytes(b []byte) *C.char { return C.CString(string(b)) }
