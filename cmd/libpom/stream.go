//go:build cgo

package main

/*
#include <stdlib.h>

// Swift registers one callback; every stream (pty/claude/pipeline/transcript)
// pushes frames through it, tagged by stream id + kind. Synchronous: the Go side
// holds the bytes only for the duration of the call, so Swift must copy on entry.
//   kind: 0=json 1=text 2=binary 3=close
typedef void (*pom_stream_cb)(int id, int kind, void* data, int len);
static pom_stream_cb pom_cb = 0;
static void pom_set_cb(pom_stream_cb cb) { pom_cb = cb; }
static void pom_emit(int id, int kind, void* data, int len) {
	if (pom_cb) pom_cb(id, kind, data, len);
}
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"sync"
	"unsafe"

	"github.com/pomelohq/pomelo/internal/commands"
)

type cgoSink struct{ id C.int }

func (s cgoSink) emit(kind C.int, b []byte) {
	if len(b) == 0 {
		C.pom_emit(s.id, kind, nil, 0)
		return
	}
	p := C.CBytes(b)
	C.pom_emit(s.id, kind, p, C.int(len(b)))
	C.free(p)
}

func (s cgoSink) SendJSON(v any) error         { b, _ := json.Marshal(v); s.emit(0, b); return nil }
func (s cgoSink) SendJSONBytes(b []byte) error { s.emit(0, b); return nil }
func (s cgoSink) SendText(b []byte) error      { s.emit(1, b); return nil }
func (s cgoSink) SendBinary(b []byte) error    { s.emit(2, b); return nil }
func (s cgoSink) Close() error                 { s.emit(3, nil); return nil }

type streamHandle struct {
	done chan struct{}
	feed func([]byte)
	stop func()
}

var (
	streamMu   sync.Mutex
	streams    = map[C.int]*streamHandle{}
	streamNext C.int
)

//export PomSetStreamCallback
func PomSetStreamCallback(cb C.pom_stream_cb) { C.pom_set_cb(cb) }

//export PomStreamPTY
func PomStreamPTY(name, wsKey *C.char, cols, rows C.int) C.int {
	mu.Lock()
	s := srv
	mu.Unlock()
	if s == nil {
		return -1
	}
	streamMu.Lock()
	streamNext++
	id := streamNext
	streamMu.Unlock()

	done := make(chan struct{})
	input, err := s.OpenPTYStream(cgoSink{id: id}, C.GoString(name), C.GoString(wsKey), int(cols), int(rows), done)
	if err != nil {
		close(done)
		return -1
	}
	streamMu.Lock()
	streams[id] = &streamHandle{done: done, feed: input.Feed}
	streamMu.Unlock()
	return id
}

//export PomStreamClaude
func PomStreamClaude(branch *C.char, isMain C.int, mode, model, role *C.char) C.int {
	mu.Lock()
	s := srv
	mu.Unlock()
	if s == nil {
		return -1
	}
	streamMu.Lock()
	streamNext++
	id := streamNext
	streamMu.Unlock()

	done := make(chan struct{})
	input := s.OpenClaudeStream(cgoSink{id: id}, done, C.GoString(branch), isMain != 0, C.GoString(mode), C.GoString(model), C.GoString(role))
	streamMu.Lock()
	streams[id] = &streamHandle{done: done, feed: func(b []byte) { input.Send(string(b)) }, stop: input.Stop}
	streamMu.Unlock()
	return id
}

//export PomStreamPrepareMain
func PomStreamPrepareMain(skipSeed C.int) C.int {
	mu.Lock()
	cfg, dir := appCfg, appDir
	mu.Unlock()
	if cfg == nil {
		return -1
	}
	streamMu.Lock()
	streamNext++
	id := streamNext
	streamMu.Unlock()

	sink := cgoSink{id: id}
	done := make(chan struct{})
	streamMu.Lock()
	streams[id] = &streamHandle{done: done}
	streamMu.Unlock()

	go func() {
		defer close(done)
		_ = commands.PrepareMainOpts(cfg, dir, commands.PrepareOpts{
			SkipSeed: skipSeed != 0,
			Emit:     func(ev commands.PrepareEvent) { _ = sink.SendJSON(ev) },
		})
		_ = sink.Close()
	}()
	return id
}

//export PomStreamCreateWorkspace
func PomStreamCreateWorkspace(branch, repos *C.char) C.int {
	mu.Lock()
	s := srv
	mu.Unlock()
	if s == nil {
		return -1
	}
	streamMu.Lock()
	streamNext++
	id := streamNext
	streamMu.Unlock()

	sink := cgoSink{id: id}
	done := make(chan struct{})
	streamMu.Lock()
	streams[id] = &streamHandle{done: done}
	streamMu.Unlock()

	go func() {
		defer close(done)
		s.StreamCreate(func(p map[string]any) { _ = sink.SendJSON(p) },
			C.GoString(branch), "", C.GoString(repos), "", "", "")
		_ = sink.Close()
	}()
	return id
}

//export PomStreamDeleteWorkspace
func PomStreamDeleteWorkspace(branch *C.char) C.int {
	mu.Lock()
	s := srv
	mu.Unlock()
	if s == nil {
		return -1
	}
	streamMu.Lock()
	streamNext++
	id := streamNext
	streamMu.Unlock()

	sink := cgoSink{id: id}
	done := make(chan struct{})
	streamMu.Lock()
	streams[id] = &streamHandle{done: done}
	streamMu.Unlock()

	go func() {
		defer close(done)
		s.StreamDelete(func(p map[string]any) { _ = sink.SendJSON(p) }, C.GoString(branch))
		_ = sink.Close()
	}()
	return id
}

//export PomStreamStop
func PomStreamStop(id C.int) {
	streamMu.Lock()
	h := streams[id]
	streamMu.Unlock()
	if h != nil && h.stop != nil {
		h.stop()
	}
}

//export PomStreamSend
func PomStreamSend(id C.int, data *C.char, length C.int) {
	feed(id, C.GoBytes(unsafe.Pointer(data), length))
}

//export PomStreamResize
func PomStreamResize(id C.int, cols, rows C.int) {
	feed(id, []byte(fmt.Sprintf(`{"__pom":"resize","cols":%d,"rows":%d}`, int(cols), int(rows))))
}

func feed(id C.int, b []byte) {
	streamMu.Lock()
	h := streams[id]
	streamMu.Unlock()
	if h != nil && h.feed != nil {
		h.feed(b)
	}
}

//export PomStreamClose
func PomStreamClose(id C.int) {
	streamMu.Lock()
	h := streams[id]
	delete(streams, id)
	streamMu.Unlock()
	if h != nil {
		close(h.done)
	}
}
