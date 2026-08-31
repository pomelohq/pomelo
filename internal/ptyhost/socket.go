package ptyhost

import (
	"bufio"
	"bytes"
	"crypto/sha1"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	gnet "github.com/shirou/gopsutil/v4/net"
	"github.com/shirou/gopsutil/v4/process"
)

const (
	frameInput   = 0x00
	frameResize  = 0x01
	framePrimary = 0x02
	frameResume  = 0x03
)

// Holder -> client output frames. The snapshot (scrollback replay) is framed so a
// reconnecting client can reset before it and resume from a byte offset, instead of
// interleaving the replay with its stale screen. After OutSnapEnd the rest of the
// connection is raw live output.
const (
	OutMeta    = 0x10 // payload: 8-byte big-endian end sequence (offset after snapshot)
	OutSnap    = 0x11 // payload: a chunk of scrollback
	OutSnapEnd = 0x12 // zero-length: snapshot done, raw live output follows
)

type OutFrame struct {
	Type    byte
	Payload []byte
}

// ReadOutFrame reads one framed output message (OutMeta/OutSnap/OutSnapEnd).
func ReadOutFrame(r *bufio.Reader) (OutFrame, error) {
	hdr := make([]byte, 3)
	if _, err := io.ReadFull(r, hdr); err != nil {
		return OutFrame{}, err
	}
	n := int(binary.BigEndian.Uint16(hdr[1:3]))
	payload := make([]byte, n)
	if n > 0 {
		if _, err := io.ReadFull(r, payload); err != nil {
			return OutFrame{}, err
		}
	}
	return OutFrame{Type: hdr[0], Payload: payload}, nil
}

func WriteResume(w io.Writer, since uint64) error {
	p := make([]byte, 8)
	binary.BigEndian.PutUint64(p, since)
	return writeFrame(w, frameResume, p)
}

func ptySockDir() string {
	if d := os.Getenv("POM_PTY_SOCK_DIR"); d != "" {
		return d
	}
	if d := os.Getenv("XDG_RUNTIME_DIR"); d != "" {
		return filepath.Join(d, "pom-pty")
	}
	return filepath.Join("/tmp", fmt.Sprintf("pom-pty-%d", os.Getuid()))
}

func SocketPath(name string) string {
	sum := sha1.Sum([]byte(name))
	return filepath.Join(ptySockDir(), hex.EncodeToString(sum[:])[:16]+".sock")
}

func DialWait(path string, d time.Duration) (net.Conn, error) {
	deadline := time.Now().Add(d)
	for {
		if c, err := net.Dial("unix", path); err == nil {
			return c, nil
		}
		if time.Now().After(deadline) {
			return nil, fmt.Errorf("timeout waiting for %s", path)
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func pidPath(name string) string {
	sum := sha1.Sum([]byte(name))
	return filepath.Join(ptySockDir(), hex.EncodeToString(sum[:])[:16]+".pid")
}

type Holder struct {
	Name string
	PID  int
}

func writeHolderPID(name string) {
	_ = os.WriteFile(pidPath(name), []byte(fmt.Sprintf("%d\n%s\n", os.Getpid(), name)), 0o644)
}

func Holders() []Holder {
	entries, err := os.ReadDir(ptySockDir())
	if err != nil {
		return nil
	}
	var out []Holder
	for _, e := range entries {
		if filepath.Ext(e.Name()) != ".pid" {
			continue
		}
		full := filepath.Join(ptySockDir(), e.Name())
		data, err := os.ReadFile(full)
		if err != nil {
			continue
		}
		f := strings.SplitN(strings.TrimSpace(string(data)), "\n", 2)
		if len(f) != 2 {
			continue
		}
		pid, err := strconv.Atoi(f[0])
		if err != nil {
			continue
		}
		if !holderAlive(pid) {
			_ = os.Remove(full)
			_ = os.Remove(SocketPath(f[1]))
			continue
		}
		out = append(out, Holder{PID: pid, Name: f[1]})
	}
	return out
}

func HolderAlive(name string) bool {
	pid := HolderPID(name)
	return pid != 0 && holderAlive(pid)
}

func holderAlive(pid int) bool {
	if pid <= 0 {
		return false
	}
	err := syscall.Kill(pid, 0)
	if err != nil && err != syscall.EPERM {
		return false
	}
	// kill(pid, 0) also succeeds for a zombie (exited, not yet reaped); a zombie
	// holder is dead, so a just-crashed service isn't reported as still running.
	return !isZombie(pid)
}

func KillHolder(name string) error {
	pid := HolderPID(name)
	if pid == 0 {
		return fmt.Errorf("no holder for %q", name)
	}
	killProcessTree(pid)
	// the holder removes its own pidfile on a clean shutdown, but a SIGKILLed tree never
	// gets there — a stale pidfile then reads as alive once the OS recycles the pid.
	_ = os.Remove(pidPath(name))
	_ = os.Remove(crashPath(name))
	return nil
}

func killProcessTree(root int) {
	pids := descendantPIDs(root)
	signalAll(pids, syscall.SIGTERM)
	deadline := time.Now().Add(4 * time.Second)
	for time.Now().Before(deadline) {
		if !anyAlive(pids) {
			return
		}
		time.Sleep(80 * time.Millisecond)
	}
	signalAll(pids, syscall.SIGKILL)
}

// KillHoldersNow tears down many holders with ONE shared short grace instead of a
// per-holder 4s wait — for quit/reap of disposable shells, where the sequential
// per-holder grace would otherwise stack up and hang the app on exit.
func KillHoldersNow(names []string) {
	var pids []int
	for _, name := range names {
		if pid := HolderPID(name); pid != 0 {
			pids = append(pids, descendantPIDs(pid)...)
			_ = os.Remove(pidPath(name))
			_ = os.Remove(crashPath(name))
		}
	}
	if len(pids) == 0 {
		return
	}
	signalAll(pids, syscall.SIGTERM)
	deadline := time.Now().Add(300 * time.Millisecond)
	for time.Now().Before(deadline) {
		if !anyAlive(pids) {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	signalAll(pids, syscall.SIGKILL)
}

func descendantPIDs(root int) []int {
	var out []int
	seen := map[int]bool{}
	queue := []int{root}
	for len(queue) > 0 {
		pid := queue[0]
		queue = queue[1:]
		if seen[pid] {
			continue
		}
		seen[pid] = true
		out = append(out, pid)
		p, err := process.NewProcess(int32(pid))
		if err != nil {
			continue
		}
		kids, err := p.Children()
		if err != nil {
			continue
		}
		for _, k := range kids {
			queue = append(queue, int(k.Pid))
		}
	}
	return out
}

func signalAll(pids []int, sig syscall.Signal) {
	for _, pid := range pids {
		_ = syscall.Kill(-pid, sig)
		_ = syscall.Kill(pid, sig)
	}
}

func anyAlive(pids []int) bool {
	for _, pid := range pids {
		if holderAlive(pid) {
			return true
		}
	}
	return false
}

func ListeningPortInTree(name string, lo, hi int) int {
	pid := HolderPID(name)
	if pid == 0 {
		return 0
	}
	for _, p := range descendantPIDs(pid) {
		proc, err := process.NewProcess(int32(p))
		if err != nil {
			continue
		}
		conns, err := proc.Connections()
		if err != nil {
			continue
		}
		for _, c := range conns {
			if c.Status == "LISTEN" && int(c.Laddr.Port) >= lo && int(c.Laddr.Port) <= hi {
				return int(c.Laddr.Port)
			}
		}
	}
	return 0
}

// ReapOrphanServices reclaims processes that escaped their holder. pomPorts is
// the set of ports Pomelo itself allocated — a port listener is only ever a reap
// candidate if it sits on one of OUR ports, so a user's own dev server (vite,
// puma, …) on any other port is never touched.
func ReapOrphanServices(scope string, pomPorts map[int]bool) []int {
	var victims []int
	seen := map[int]bool{}
	add := func(pid int) {
		if pid > 1 && !seen[pid] {
			seen[pid] = true
			victims = append(victims, pid)
		}
	}

	if all, err := process.Processes(); err == nil {
		byName := map[string][]int{}
		for _, p := range all {
			cmd, _ := p.Cmdline()
			if name := holderNameFromCmd(cmd); name != "" {
				byName[name] = append(byName[name], int(p.Pid))
			}
		}
		for name, pids := range byName {
			if len(pids) < 2 {
				continue
			}
			keep := HolderPID(name)
			found := false
			for _, pid := range pids {
				if pid == keep {
					found = true
				}
			}
			if !found {
				keep = pids[0]
				for _, pid := range pids {
					if pid > keep {
						keep = pid
					}
				}
			}
			for _, pid := range pids {
				if pid != keep {
					add(pid)
				}
			}
		}
	}

	owned := ownedPIDs()
	const lo, hi = 10000, 65535
	if conns, err := gnet.Connections("tcp"); err == nil {
		for _, c := range conns {
			pid := int(c.Pid)
			if c.Status != "LISTEN" || pid <= 1 || owned[pid] {
				continue
			}
			if c.Laddr.Port < lo || c.Laddr.Port > hi {
				continue
			}
			if !pomPorts[int(c.Laddr.Port)] { // only reclaim ports WE allocated
				continue
			}
			if p, err := process.NewProcess(int32(pid)); err == nil {
				if cmd, _ := p.Cmdline(); looksLikeServiceRuntime(cmd) {
					add(pid)
				}
			}
		}
	}

	if len(victims) == 0 {
		return nil
	}
	all := map[int]bool{}
	for _, v := range victims {
		for _, d := range descendantPIDs(v) {
			all[d] = true
		}
	}
	pids := make([]int, 0, len(all))
	for p := range all {
		pids = append(pids, p)
	}
	signalAll(pids, syscall.SIGTERM)
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) && anyAlive(pids) {
		time.Sleep(80 * time.Millisecond)
	}
	signalAll(pids, syscall.SIGKILL)
	return victims
}

func holderNameFromCmd(cmd string) string {
	f := strings.Fields(cmd)
	for i := 0; i+2 < len(f); i++ {
		if f[i] == "pty" && f[i+1] == "run" {
			return f[i+2]
		}
	}
	return ""
}

func ownedPIDs() map[int]bool {
	set := map[int]bool{}
	all, err := process.Processes()
	if err != nil {
		return set
	}
	for _, p := range all {
		cmd, _ := p.Cmdline()
		if holderNameFromCmd(cmd) == "" {
			continue
		}
		for _, d := range descendantPIDs(int(p.Pid)) {
			set[d] = true
		}
	}
	return set
}

func looksLikeServiceRuntime(cmd string) bool {
	for _, pat := range []string{
		"serve -s", "vite", "next dev", "next start", "node dist/",
		"puma", "sidekiq", "rails server", "watchexec", "prisma", "nest ",
	} {
		if strings.Contains(cmd, pat) {
			return true
		}
	}
	return false
}

func HolderPID(name string) int {
	data, err := os.ReadFile(pidPath(name))
	if err != nil {
		return 0
	}
	f := strings.SplitN(strings.TrimSpace(string(data)), "\n", 2)
	pid, _ := strconv.Atoi(f[0])
	return pid
}

func Snapshot(name string, timeout time.Duration) []byte {
	c, err := net.Dial("unix", SocketPath(name))
	if err != nil {
		return nil
	}
	defer c.Close()
	_ = WriteResume(c, 0)
	_ = c.SetReadDeadline(time.Now().Add(timeout))
	r := bufio.NewReader(c)
	var buf []byte
	for len(buf) < ringCap {
		fr, err := ReadOutFrame(r)
		if err != nil {
			break
		}
		if fr.Type == OutSnap {
			buf = append(buf, fr.Payload...)
		} else if fr.Type == OutSnapEnd {
			break
		}
	}
	return buf
}

func Serve(ln net.Listener, o StartOpts) (*Session, error) {
	s, err := Start(o)
	if err != nil {
		return nil, err
	}
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go serveConn(s, conn)
		}
	}()
	go func() { <-s.Done(); _ = ln.Close() }()
	return s, nil
}

func ListenAndServe(name string, o StartOpts) (*Session, net.Listener, error) {
	path := SocketPath(name)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, nil, err
	}
	_ = os.Remove(path)
	ln, err := net.Listen("unix", path)
	if err != nil {
		return nil, nil, err
	}
	_ = os.Remove(crashPath(name)) // fresh holder starts with no stale crash record
	o.OnExit = func(scrollback []byte, exitErr error) { writeCrashLog(name, scrollback, exitErr) }
	s, err := Serve(ln, o)
	if err != nil {
		_ = ln.Close()
		_ = os.Remove(path)
		return nil, nil, err
	}
	writeHolderPID(name)
	go func() {
		<-s.Done()
		_ = os.Remove(path)
		_ = os.Remove(pidPath(name))
	}()
	return s, ln, nil
}

func crashPath(name string) string {
	sum := sha1.Sum([]byte(name))
	return filepath.Join(ptySockDir(), hex.EncodeToString(sum[:])[:16]+".crash")
}

func exitedAbnormally(exitErr error) bool {
	return exitErr != nil && !strings.Contains(exitErr.Error(), "signal: terminated")
}

func writeCrashLog(name string, scrollback []byte, exitErr error) {
	kind, status := "STOP", "exited cleanly"
	if exitErr != nil {
		status = "exited: " + exitErr.Error()
	}
	if exitedAbnormally(exitErr) {
		kind = "CRASH"
	}
	hdr := fmt.Sprintf("%s\t%s · %s\n", kind, name, status)
	_ = os.WriteFile(crashPath(name), append([]byte(hdr), scrollback...), 0o644)
}

func CrashInfo(name string) (crashed bool, output []byte) {
	data, err := os.ReadFile(crashPath(name))
	if err != nil || len(data) == 0 {
		return false, nil
	}
	nl := indexByte(data, '\n')
	if nl < 0 {
		return false, data
	}
	return strings.HasPrefix(string(data[:nl]), "CRASH"), data[nl+1:]
}

// stripDeviceQueries scrubs terminal query sequences (DA / DSR / DECRQM /
// XTVERSION / OSC color queries) from a scrollback snapshot. Programs emit these
// to probe the terminal; replaying them when a tab re-attaches makes the emulator
// answer a second time and injects the reply at the shell prompt (e.g. a stray
// `2026;2$y`). Only the historical snapshot is scrubbed — the live stream keeps
// every byte, so real queries still get answered.
func stripDeviceQueries(b []byte) []byte {
	out := make([]byte, 0, len(b))
	for i := 0; i < len(b); {
		if b[i] == 0x1b && i+1 < len(b) {
			switch b[i+1] {
			case '[': // CSI
				j := i + 2
				for j < len(b) && b[j] >= 0x20 && b[j] <= 0x3f {
					j++
				}
				if j < len(b) && csiIsQuery(b[i:j+1]) {
					i = j + 1
					continue
				}
			case ']': // OSC — drop color/clipboard queries (contain ";?")
				j := i + 2
				for j < len(b) {
					if b[j] == 0x07 {
						j++
						break
					}
					if b[j] == 0x1b && j+1 < len(b) && b[j+1] == '\\' {
						j += 2
						break
					}
					j++
				}
				if bytes.Contains(b[i:j], []byte(";?")) {
					i = j
					continue
				}
			}
		}
		out = append(out, b[i])
		i++
	}
	return out
}

func csiIsQuery(seq []byte) bool {
	switch seq[len(seq)-1] {
	case 'c', 'n': // Device Attributes, Device Status Report
		return true
	case 'p': // DECRQM: CSI [?]Ps $ p
		return bytes.IndexByte(seq, '$') >= 0
	case 'q': // XTVERSION: CSI > Ps q (DECSCUSR uses a SP intermediate, keep it)
		return bytes.IndexByte(seq, '>') >= 0
	}
	return false
}

func serveConn(s *Session, conn net.Conn) {
	defer conn.Close()
	r := bufio.NewReader(conn)

	since, pending := readLeadingResume(r, conn)

	snap, endSeq, out, cancel := s.SubscribeSince(since)
	defer cancel()

	clientID := s.AddClient()
	defer s.RemoveClient(clientID)

	seqBuf := make([]byte, 8)
	binary.BigEndian.PutUint64(seqBuf, endSeq)
	if writeFrame(conn, OutMeta, seqBuf) != nil {
		return
	}
	strip := stripDeviceQueries(snap)
	for len(strip) > 0 {
		n := len(strip)
		if n > 32*1024 {
			n = 32 * 1024
		}
		if writeFrame(conn, OutSnap, strip[:n]) != nil {
			return
		}
		strip = strip[n:]
	}
	if writeFrame(conn, OutSnapEnd, nil) != nil {
		return
	}

	go func() {
		if pending != nil {
			applyInputFrame(s, clientID, pending.Type, pending.Payload)
		}
		hdr := make([]byte, 3)
		for {
			if _, err := io.ReadFull(r, hdr); err != nil {
				return
			}
			n := int(binary.BigEndian.Uint16(hdr[1:3]))
			payload := make([]byte, n)
			if _, err := io.ReadFull(r, payload); err != nil {
				return
			}
			applyInputFrame(s, clientID, hdr[0], payload)
		}
	}()

	for chunk := range out {
		if _, err := conn.Write(chunk); err != nil {
			return
		}
	}
}

// readLeadingResume reads the client's optional resume offset (sent first). Callers
// that send no frame promptly (e.g. a one-shot Snapshot reader) fall through after a
// short wait and get the full snapshot; a non-resume first frame is handed back so
// the input loop still applies it.
func readLeadingResume(r *bufio.Reader, conn net.Conn) (since uint64, pending *OutFrame) {
	_ = conn.SetReadDeadline(time.Now().Add(40 * time.Millisecond))
	defer conn.SetReadDeadline(time.Time{})
	hdr := make([]byte, 3)
	if _, err := io.ReadFull(r, hdr); err != nil {
		return 0, nil
	}
	n := int(binary.BigEndian.Uint16(hdr[1:3]))
	payload := make([]byte, n)
	if n > 0 {
		if _, err := io.ReadFull(r, payload); err != nil {
			return 0, nil
		}
	}
	if hdr[0] == frameResume && len(payload) == 8 {
		return binary.BigEndian.Uint64(payload), nil
	}
	return 0, &OutFrame{Type: hdr[0], Payload: payload}
}

func applyInputFrame(s *Session, clientID uint64, typ byte, payload []byte) {
	switch typ {
	case frameInput:
		_, _ = s.Write(payload)
	case frameResize:
		if len(payload) == 4 {
			s.ClientSize(clientID, int(binary.BigEndian.Uint16(payload[0:2])),
				int(binary.BigEndian.Uint16(payload[2:4])))
		}
	case framePrimary:
		s.SetClientPrimary(clientID)
	}
}

func writeFrame(w io.Writer, typ byte, payload []byte) error {
	hdr := make([]byte, 3)
	hdr[0] = typ
	binary.BigEndian.PutUint16(hdr[1:3], uint16(len(payload)))
	if _, err := w.Write(hdr); err != nil {
		return err
	}
	_, err := w.Write(payload)
	return err
}

func WriteInput(w io.Writer, p []byte) error { return writeFrame(w, frameInput, p) }

func WritePrimary(w io.Writer) error { return writeFrame(w, framePrimary, nil) }
func WriteResize(w io.Writer, cols, rows int) error {
	p := make([]byte, 4)
	binary.BigEndian.PutUint16(p[0:2], uint16(cols))
	binary.BigEndian.PutUint16(p[2:4], uint16(rows))
	return writeFrame(w, frameResize, p)
}

func Attach(conn net.Conn, in io.Reader, out io.Writer, resize <-chan [2]int, detach byte) error {
	done := make(chan error, 1)

	go func() {
		_, err := io.Copy(out, conn)
		done <- err
	}()
	go func() {
		for sz := range resize {
			if WriteResize(conn, sz[0], sz[1]) != nil {
				return
			}
		}
	}()
	go func() {
		buf := make([]byte, 4096)
		for {
			n, err := in.Read(buf)
			if n > 0 {
				if i := indexByte(buf[:n], detach); i >= 0 {
					if i > 0 {
						_ = WriteInput(conn, buf[:i])
					}
					done <- nil
					return
				}
				if WriteInput(conn, buf[:n]) != nil {
					return
				}
			}
			if err != nil {
				done <- err
				return
			}
		}
	}()

	return <-done
}

func indexByte(b []byte, c byte) int {
	for i := range b {
		if b[i] == c {
			return i
		}
	}
	return -1
}
