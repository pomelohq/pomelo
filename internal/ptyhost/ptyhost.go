package ptyhost

import (
	"os"
	"strings"
	"sync"
	"time"

	"github.com/aymanbagabas/go-pty"
)

const ringCap = 256 * 1024

type StartOpts struct {
	Argv       []string
	Dir        string
	Env        []string
	Cols, Rows int
	OnExit     func(scrollback []byte, err error)
}

type Session struct {
	pty pty.Pty
	cmd *pty.Cmd

	mu    sync.Mutex
	ring  []byte
	total uint64
	subs  map[chan []byte]struct{}

	szMu           sync.Mutex
	szCols, szRows int

	clMu      sync.Mutex
	clients   map[uint64]clientSz
	clientSeq uint64

	closePty sync.Once
	done     chan struct{}
	readDone chan struct{}
	onExit   func([]byte, error)
	waitErr  error
}

func Start(o StartOpts) (*Session, error) {
	p, err := pty.New()
	if err != nil {
		return nil, err
	}
	cols, rows := o.Cols, o.Rows
	if cols <= 0 {
		cols = 80
	}
	if rows <= 0 {
		rows = 24
	}
	_ = p.Resize(cols, rows)

	c := p.Command(o.Argv[0], o.Argv[1:]...)
	c.Dir = o.Dir
	c.Env = ensureTerm(o.Env)
	if err := c.Start(); err != nil {
		_ = p.Close()
		return nil, err
	}

	s := &Session{pty: p, cmd: c, subs: map[chan []byte]struct{}{}, clients: map[uint64]clientSz{}, done: make(chan struct{}), readDone: make(chan struct{}), onExit: o.OnExit, szCols: cols, szRows: rows}
	go s.readLoop()
	go s.reap()
	return s, nil
}

func (s *Session) readLoop() {
	defer close(s.readDone)
	buf := make([]byte, 32*1024)
	var carry []byte
	for {
		n, err := s.pty.Read(buf)
		if n > 0 {
			cleaned, tail := s.answerQueries(append(carry, buf[:n]...))
			carry = tail
			if len(cleaned) > 0 {
				s.publish(cleaned)
			}
		}
		if err != nil {
			return
		}
	}
}

func (s *Session) answerQueries(b []byte) (cleaned, carry []byte) {
	var reply []byte
	out := make([]byte, 0, len(b))
	i := 0
	for i < len(b) {
		if b[i] != 0x1b {
			out = append(out, b[i])
			i++
			continue
		}
		rest := b[i:]
		n := 0
		switch {
		case hasPrefix(rest, "\x1b[>0q"), hasPrefix(rest, "\x1b[>q"):
			reply = append(reply, "\x1bP>|ptyhost 1.0\x1b\\"...)
			n = queryLen(rest, 'q')
		case hasPrefix(rest, "\x1b[?u"):
			reply = append(reply, "\x1b[?0u"...)
			n = 4
		case hasPrefix(rest, "\x1b[>c"), hasPrefix(rest, "\x1b[>0c"):
			reply = append(reply, "\x1b[>0;276;0c"...)
			n = queryLen(rest, 'c')
		case hasPrefix(rest, "\x1b[c"), hasPrefix(rest, "\x1b[0c"):
			reply = append(reply, "\x1b[?64;1;2;6;9;15;18;21;22c"...)
			n = queryLen(rest, 'c')
		case hasPrefix(rest, "\x1b[5n"):
			reply = append(reply, "\x1b[0n"...)
			n = 4
		case hasPrefix(rest, "\x1b[6n"):
			reply = append(reply, "\x1b[1;1R"...)
			n = 4
		default:
			if isPartialQuery(rest) {
				return out, append([]byte(nil), rest...)
			}
			out = append(out, b[i])
			i++
			continue
		}
		i += n
	}
	if len(reply) > 0 {
		_, _ = s.Write(reply)
	}
	return out, nil
}

func hasPrefix(b []byte, p string) bool {
	if len(b) < len(p) {
		return false
	}
	for i := 0; i < len(p); i++ {
		if b[i] != p[i] {
			return false
		}
	}
	return true
}

func queryLen(b []byte, final byte) int {
	for i := 2; i < len(b) && i < 12; i++ {
		if b[i] == final {
			return i + 1
		}
	}
	return 3
}

func isPartialQuery(b []byte) bool {
	return len(b) < 6 && (hasPrefix(b, "\x1b") && !hasCSIFinal(b))
}

func hasCSIFinal(b []byte) bool {
	for _, c := range b {
		if c >= 0x40 && c <= 0x7e && c != '[' {
			return true
		}
	}
	return false
}

func (s *Session) publish(chunk []byte) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.total += uint64(len(chunk))
	s.ring = append(s.ring, chunk...)
	if len(s.ring) > ringCap {
		s.ring = append(s.ring[:0], s.ring[len(s.ring)-ringCap:]...)
	}
	for ch := range s.subs {
		select {
		case ch <- chunk:
		default:
		}
	}
}

func (s *Session) reap() {
	s.waitErr = s.cmd.Wait()
	select {
	case <-s.readDone:
	case <-time.After(500 * time.Millisecond):
	}
	s.shutPty()
	if s.onExit != nil {
		s.onExit(s.Scrollback(), s.waitErr)
	}
	s.mu.Lock()
	for ch := range s.subs {
		delete(s.subs, ch)
		close(ch)
	}
	s.mu.Unlock()
	close(s.done)
}

func (s *Session) shutPty() { s.closePty.Do(func() { _ = s.pty.Close() }) }

func ensureTerm(env []string) []string {
	if env == nil {
		env = os.Environ()
	}
	has := func(key string) bool {
		for _, e := range env {
			if strings.HasPrefix(e, key+"=") {
				return true
			}
		}
		return false
	}
	out := env[:len(env):len(env)]
	if !has("TERM") {
		out = append(out, "TERM=xterm-256color")
	}
	if !has("COLORTERM") {
		out = append(out, "COLORTERM=truecolor")
	}
	return out
}

func (s *Session) Subscribe() (snapshot []byte, out <-chan []byte, cancel func()) {
	snap, _, out, cancel := s.SubscribeSince(0)
	return snap, out, cancel
}

// SubscribeSince returns the scrollback tail after the given byte offset (the full
// ring when the offset is stale or zero) plus endSeq, the absolute offset of the end
// of that snapshot, so the caller can resume from endSeq on its next connect.
func (s *Session) SubscribeSince(since uint64) (snapshot []byte, endSeq uint64, out <-chan []byte, cancel func()) {
	ch := make(chan []byte, 256)
	s.mu.Lock()
	endSeq = s.total
	baseSeq := s.total - uint64(len(s.ring))
	if since > baseSeq && since <= s.total {
		snapshot = append([]byte(nil), s.ring[since-baseSeq:]...)
	} else {
		snapshot = append([]byte(nil), s.ring...)
	}
	s.subs[ch] = struct{}{}
	s.mu.Unlock()

	cancel = func() {
		s.mu.Lock()
		if _, ok := s.subs[ch]; ok {
			delete(s.subs, ch)
			close(ch)
		}
		s.mu.Unlock()
	}
	return snapshot, endSeq, ch, cancel
}

func (s *Session) Scrollback() []byte {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]byte, len(s.ring))
	copy(out, s.ring)
	return out
}

func (s *Session) Write(p []byte) (int, error) { return s.pty.Write(p) }

func (s *Session) Resize(cols, rows int) error {
	s.szMu.Lock()
	if cols == s.szCols && rows == s.szRows {
		s.szMu.Unlock()
		return nil
	}
	s.szCols, s.szRows = cols, rows
	s.szMu.Unlock()
	return s.pty.Resize(cols, rows)
}

type clientSz struct {
	cols, rows int
	primary    bool
}

func (s *Session) AddClient() uint64 {
	s.clMu.Lock()
	defer s.clMu.Unlock()
	s.clientSeq++
	id := s.clientSeq
	s.clients[id] = clientSz{}
	return id
}

func (s *Session) RemoveClient(id uint64) {
	s.clMu.Lock()
	delete(s.clients, id)
	c, r := s.minSizeLocked()
	s.clMu.Unlock()
	if c > 0 && r > 0 {
		_ = s.Resize(c, r)
	}
}

func (s *Session) ClientSize(id uint64, cols, rows int) {
	if cols <= 0 || rows <= 0 {
		return
	}
	s.clMu.Lock()
	c := s.clients[id]
	c.cols, c.rows = cols, rows
	s.clients[id] = c
	nc, nr := s.minSizeLocked()
	s.clMu.Unlock()
	if nc > 0 && nr > 0 {
		_ = s.Resize(nc, nr)
	}
}

func (s *Session) SetClientPrimary(id uint64) {
	s.clMu.Lock()
	c := s.clients[id]
	c.primary = true
	s.clients[id] = c
	nc, nr := s.minSizeLocked()
	s.clMu.Unlock()
	if nc > 0 && nr > 0 {
		_ = s.Resize(nc, nr)
	}
}

func (s *Session) minSizeLocked() (cols, rows int) {
	primaryOnly := false
	for _, c := range s.clients {
		if c.primary && c.cols > 0 && c.rows > 0 {
			primaryOnly = true
			break
		}
	}
	for _, c := range s.clients {
		if c.cols <= 0 || c.rows <= 0 || (primaryOnly && !c.primary) {
			continue
		}
		if cols == 0 || c.cols < cols {
			cols = c.cols
		}
		if rows == 0 || c.rows < rows {
			rows = c.rows
		}
	}
	return
}

func (s *Session) Done() <-chan struct{} { return s.done }

func (s *Session) Wait() error { <-s.done; return s.waitErr }

func (s *Session) Close() {
	if s.cmd.Process != nil {
		_ = s.cmd.Process.Kill()
	}
	s.shutPty()
}
