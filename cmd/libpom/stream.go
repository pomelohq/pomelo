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

func nextStreamID() C.int {
	streamMu.Lock()
	streamNext++
	id := streamNext
	streamMu.Unlock()
	return id
}

// PomSubscribe opens a live stream routed by topic (the streaming analog of
// PomQuery/PomCommand): one export instead of one per stream kind. Params carry
// the topic's args as JSON. Frames arrive via the registered stream callback.
//
//export PomSubscribe
func PomSubscribe(topic, paramsJSON *C.char) C.int {
	var p map[string]any
	_ = json.Unmarshal([]byte(C.GoString(paramsJSON)), &p)
	str := func(k string) string { s, _ := p[k].(string); return s }
	flag := func(k string) bool { b, _ := p[k].(bool); return b }
	num := func(k string) int { f, _ := p[k].(float64); return int(f) }

	switch C.GoString(topic) {
	case "pty":
		mu.Lock()
		s := srv
		mu.Unlock()
		if s == nil {
			return -1
		}
		id := nextStreamID()
		done := make(chan struct{})
		input, err := s.OpenPTYStream(cgoSink{id: id}, str("name"), str("ws_key"), num("cols"), num("rows"), uint64(num("since")), done)
		if err != nil {
			close(done)
			return -1
		}
		streamMu.Lock()
		streams[id] = &streamHandle{done: done, feed: input.Feed}
		streamMu.Unlock()
		return id
	case "claude":
		mu.Lock()
		s := srv
		mu.Unlock()
		if s == nil {
			return -1
		}
		id := nextStreamID()
		done := make(chan struct{})
		input := s.OpenClaudeStream(cgoSink{id: id}, done, str("branch"), flag("is_main"), str("mode"), str("model"), str("role"))
		streamMu.Lock()
		streams[id] = &streamHandle{done: done, feed: func(b []byte) { input.Send(string(b)) }, stop: input.Stop}
		streamMu.Unlock()
		return id
	case "prepare_main":
		mu.Lock()
		cfg, dir := appCfg, appDir
		mu.Unlock()
		if cfg == nil {
			return -1
		}
		id := nextStreamID()
		sink := cgoSink{id: id}
		done := make(chan struct{})
		streamMu.Lock()
		streams[id] = &streamHandle{done: done}
		streamMu.Unlock()
		go func() {
			defer close(done)
			_ = commands.PrepareMainOpts(cfg, dir, commands.PrepareOpts{
				SkipSeed: flag("skip_seed"),
				Emit:     func(ev commands.PrepareEvent) { _ = sink.SendJSON(ev) },
			})
			_ = sink.Close()
		}()
		return id
	case "create_workspace":
		mu.Lock()
		s := srv
		mu.Unlock()
		if s == nil {
			return -1
		}
		id := nextStreamID()
		sink := cgoSink{id: id}
		done := make(chan struct{})
		streamMu.Lock()
		streams[id] = &streamHandle{done: done}
		streamMu.Unlock()
		go func() {
			defer close(done)
			s.StreamCreate(func(pp map[string]any) { _ = sink.SendJSON(pp) },
				str("branch"), "", str("repos"), "", "", "")
			_ = sink.Close()
		}()
		return id
	case "delete_workspace":
		mu.Lock()
		s := srv
		mu.Unlock()
		if s == nil {
			return -1
		}
		id := nextStreamID()
		sink := cgoSink{id: id}
		done := make(chan struct{})
		streamMu.Lock()
		streams[id] = &streamHandle{done: done}
		streamMu.Unlock()
		go func() {
			defer close(done)
			s.StreamDelete(func(pp map[string]any) { _ = sink.SendJSON(pp) }, str("branch"))
			_ = sink.Close()
		}()
		return id
	case "add_repo":
		mu.Lock()
		s := srv
		mu.Unlock()
		if s == nil {
			return -1
		}
		id := nextStreamID()
		sink := cgoSink{id: id}
		done := make(chan struct{})
		streamMu.Lock()
		streams[id] = &streamHandle{done: done}
		streamMu.Unlock()
		go func() {
			defer close(done)
			s.StreamAddRepo(func(pp map[string]any) { _ = sink.SendJSON(pp) }, str("branch"), flag("is_main"), str("repos"))
			_ = sink.Close()
		}()
		return id
	}
	return -1
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
