package core

import (
	"fmt"
	"strconv"

	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/stream"
)

type PtyInput struct {
	sock interface{ Write([]byte) (int, error) }
}

func (p PtyInput) Feed(data []byte) {
	if _, ok := parseAckControl(data); ok {
		return
	}
	if c, ok := parseResizeControl(data); ok {
		cw, _ := strconv.Atoi(c.Cols)
		ch, _ := strconv.Atoi(c.Rows)
		if cw > 0 && ch > 0 {
			_ = ptyhost.WriteResize(p.sock, cw, ch)
		}
		return
	}
	_ = ptyhost.WriteInput(p.sock, data)
}

func (s *Server) OpenPTYStream(sink stream.Sink, name, wsKey string, cols, rows int, since uint64, done <-chan struct{}) (PtyInput, error) {
	if name == "" {
		name = "shell"
	}
	sock, err := s.dialOrSpawnHolder(name, cols, rows, s.ptyCwd(wsKey))
	if err != nil {
		return PtyInput{}, err
	}
	_ = ptyhost.WriteResume(sock, since)
	_ = ptyhost.WritePrimary(sock)
	if cols > 0 && rows > 0 {
		_ = ptyhost.WriteResize(sock, cols, rows)
		_ = sink.SendText([]byte(fmt.Sprintf(`{"type":"size","cols":%d,"rows":%d}`, cols, rows)))
	}
	go pumpPtyOutput(sock, sink)
	go func() { <-done; _ = sock.Close() }()
	return PtyInput{sock: sock}, nil
}
