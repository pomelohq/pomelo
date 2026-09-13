//go:build darwin && cgo

package services

/*
#cgo LDFLAGS: -framework CoreServices
#include <stdlib.h>
#include "fswatch_darwin.h"
*/
import "C"

import (
	"errors"
	"sync"
	"time"
	"unsafe"
)

const fsWatchDebounce = 300 * time.Millisecond

type fsWatcher struct {
	onChange func()

	mu    sync.Mutex
	timer *time.Timer
	dead  bool
}

var (
	fsWatchMu   sync.Mutex
	fsWatchers  = map[C.int]*fsWatcher{}
	fsWatchNext C.int
)

//export pomFSEventsCallback
func pomFSEventsCallback(handle C.int, path *C.char) {
	fsWatchMu.Lock()
	w := fsWatchers[handle]
	fsWatchMu.Unlock()
	if w == nil || FSWatchIgnoredPath(C.GoString(path)) {
		return
	}
	w.schedule()
}

func (w *fsWatcher) schedule() {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.dead {
		return
	}
	if w.timer != nil {
		w.timer.Reset(fsWatchDebounce)
		return
	}
	w.timer = time.AfterFunc(fsWatchDebounce, func() {
		w.mu.Lock()
		dead := w.dead
		w.mu.Unlock()
		if !dead {
			w.onChange()
		}
	})
}

func (w *fsWatcher) kill() {
	w.mu.Lock()
	w.dead = true
	if w.timer != nil {
		w.timer.Stop()
		w.timer = nil
	}
	w.mu.Unlock()
}

func WatchTree(root string, onChange func()) (func(), error) {
	if root == "" || onChange == nil {
		return func() {}, errors.New("fswatch: root and onChange are required")
	}

	fsWatchMu.Lock()
	fsWatchNext++
	handle := fsWatchNext
	w := &fsWatcher{onChange: onChange}
	fsWatchers[handle] = w
	fsWatchMu.Unlock()

	cRoot := C.CString(root)
	defer C.free(unsafe.Pointer(cRoot))

	queue := C.pom_fsevents_queue()
	stream := C.pom_fsevents_start(cRoot, handle, queue)
	if stream == nil {
		C.pom_fsevents_release_queue(queue)
		fsWatchMu.Lock()
		delete(fsWatchers, handle)
		fsWatchMu.Unlock()
		return func() {}, errors.New("fswatch: could not start FSEvents stream for " + root)
	}

	var once sync.Once
	return func() {
		once.Do(func() {
			C.pom_fsevents_stop(stream)
			C.pom_fsevents_release_queue(queue)
			w.kill()
			fsWatchMu.Lock()
			delete(fsWatchers, handle)
			fsWatchMu.Unlock()
		})
	}, nil
}
