package core

import (
	"bufio"
	"encoding/binary"
	"fmt"
	"net"

	"github.com/pomelohq/pomelo/internal/ptyhost"
	"github.com/pomelohq/pomelo/internal/stream"
)

// pumpPtyOutput translates the holder's framed snapshot prefix into control frames
// the client can act on (reset before the replay, then a synced offset it resumes
// from), then forwards raw live output. Clients that only consume binary frames
// (the desktop terminal) ignore the controls and see the same bytes as before.
func pumpPtyOutput(sock net.Conn, sink stream.Sink) {
	r := bufio.NewReader(sock)
	var endSeq uint64
	sentReset := false
	for {
		fr, err := ptyhost.ReadOutFrame(r)
		if err != nil {
			_ = sink.SendText([]byte(`{"type":"exit"}`))
			_ = sink.Close()
			return
		}
		switch fr.Type {
		case ptyhost.OutMeta:
			if len(fr.Payload) == 8 {
				endSeq = binary.BigEndian.Uint64(fr.Payload)
			}
		case ptyhost.OutSnap:
			if !sentReset {
				sentReset = true
				_ = sink.SendJSONBytes([]byte(`{"type":"reset"}`))
			}
			if sink.SendBinary(fr.Payload) != nil {
				return
			}
		case ptyhost.OutSnapEnd:
			_ = sink.SendJSONBytes([]byte(fmt.Sprintf(`{"type":"synced","seq":%d}`, endSeq)))
			pumpLive(r, sink)
			return
		}
	}
}

func pumpLive(r *bufio.Reader, sink stream.Sink) {
	buf := make([]byte, 32*1024)
	for {
		n, err := r.Read(buf)
		if n > 0 {
			if sink.SendBinary(buf[:n]) != nil {
				return
			}
		}
		if err != nil {
			_ = sink.SendText([]byte(`{"type":"exit"}`))
			_ = sink.Close()
			return
		}
	}
}

func forwardBytes(sink stream.Sink, ch <-chan []byte, done <-chan struct{}) {
	for {
		select {
		case b := <-ch:
			if sink.SendJSONBytes(b) != nil {
				return
			}
		case <-done:
			return
		}
	}
}
