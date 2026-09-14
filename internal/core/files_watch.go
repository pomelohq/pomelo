package core

import (
	"bytes"

	"github.com/pomelohq/pomelo/internal/services"
	"github.com/pomelohq/pomelo/internal/stream"
)

func (s *Server) OpenFilesStream(sink stream.Sink, branch string, isMain bool, done <-chan struct{}) {
	root := s.workspaceRoot(branch, isMain)
	if root == "" {
		_ = sink.SendJSONBytes([]byte(`[]`))
		_ = sink.Close()
		return
	}

	var last []byte
	changed := make(chan struct{}, 1)

	push := func() {
		listing := s.ListWorkspaceFiles(branch, isMain)
		if bytes.Equal(listing, last) {
			return
		}
		last = listing
		_ = sink.SendJSONBytes(listing)
	}

	push()

	stop, err := services.WatchTree(root, func() {
		select {
		case changed <- struct{}{}:
		default:
		}
	})
	if err != nil {
		_ = sink.Close()
		return
	}

	go func() {
		defer stop()
		defer func() { _ = sink.Close() }()
		for {
			select {
			case <-done:
				return
			case <-changed:
				push()
			}
		}
	}()
}
