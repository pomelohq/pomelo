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

//export PomNMStoreProgress
func PomNMStoreProgress() *C.char {
	s := server()
	if s == nil {
		return C.CString("")
	}
	return C.CString(s.NMStoreProgress())
}

//export PomSessionCreate
func PomSessionCreate(reqJSON *C.char) *C.char {
	var req core.CreateSessionReq
	if err := json.Unmarshal([]byte(C.GoString(reqJSON)), &req); err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": "bad json"})
	}
	dir, err := core.ScaffoldSession(req)
	if err != nil {
		return bindingJSON(map[string]any{"ok": false, "error": err.Error()})
	}
	return bindingJSON(map[string]any{"ok": true, "name": req.Name, "path": dir})
}

//export PomPRAll
func PomPRAll() *C.char {
	s := server()
	if s == nil {
		return C.CString(`{}`)
	}
	return bindingBytes(s.PRAllPRs())
}

//export PomPRWorkspace
func PomPRWorkspace(branch *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"prs":[]}`)
	}
	return bindingBytes(s.PRWorkspacePRs(C.GoString(branch), isMain != 0))
}

//export PomPRDetail
func PomPRDetail(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"pr":null}`)
	}
	return bindingBytes(s.PRDetail(C.GoString(branch), C.GoString(repo), isMain != 0))
}

//export PomPRComments
func PomPRComments(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"comments":[]}`)
	}
	return bindingBytes(s.PRComments(C.GoString(branch), C.GoString(repo), isMain != 0))
}

//export PomPRCommits
func PomPRCommits(branch, repo, base *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"commits":[]}`)
	}
	return bindingJSON(s.PRCommits(C.GoString(branch), C.GoString(repo), C.GoString(base), isMain != 0))
}

//export PomPRDiff
func PomPRDiff(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString("")
	}
	out, err := s.PRDiff(C.GoString(branch), C.GoString(repo), isMain != 0)
	if err != nil {
		return C.CString("")
	}
	return bindingBytes(out)
}

//export PomWorkspaceLocalChanges
func PomWorkspaceLocalChanges(branch *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString(`{"repos":[]}`)
	}
	return bindingBytes(s.WorkspaceLocalChanges(C.GoString(branch), isMain != 0))
}

//export PomLocalDiff
func PomLocalDiff(branch, repo *C.char, isMain C.int) *C.char {
	s := server()
	if s == nil {
		return C.CString("")
	}
	out, err := s.LocalDiff(C.GoString(branch), C.GoString(repo), isMain != 0)
	if err != nil {
		return C.CString("")
	}
	return bindingBytes(out)
}
