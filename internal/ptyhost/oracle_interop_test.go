package ptyhost

import (
	"bufio"
	"bytes"
	"crypto/sha1"
	"encoding/hex"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// TestOracleClient attaches to a holder started by the Rust port, types into it and records what came back,
// proving the two implementations speak the same protocol. TestMain overrides POM_PTY_SOCK_DIR, so the
// directory comes in through its own variable.
func TestOracleClient(t *testing.T) {
	dir, name, out := os.Getenv("POM_ORACLE_SOCK_DIR"), os.Getenv("POM_ORACLE_PTY_NAME"), os.Getenv("POM_ORACLE_OUT")
	if dir == "" || name == "" || out == "" {
		t.Skip("interop check only runs when driven by the parity tests")
	}
	sum := sha1.Sum([]byte(name))
	conn, err := net.Dial("unix", filepath.Join(dir, hex.EncodeToString(sum[:])[:16]+".sock"))
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	if err := WriteResume(conn, 0); err != nil {
		t.Fatal(err)
	}
	r := bufio.NewReader(conn)
	var snapshot []byte
	for {
		fr, err := ReadOutFrame(r)
		if err != nil {
			t.Fatal(err)
		}
		if fr.Type == OutSnap {
			snapshot = append(snapshot, fr.Payload...)
		}
		if fr.Type == OutSnapEnd {
			break
		}
	}
	if err := WriteInput(conn, []byte("go-says-hi\n")); err != nil {
		t.Fatal(err)
	}
	_ = conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	live := []byte{}
	buf := make([]byte, 4096)
	for !bytes.Contains(live, []byte("go-says-hi")) {
		n, err := r.Read(buf)
		live = append(live, buf[:n]...)
		if err != nil {
			t.Fatalf("read: %v; got %q", err, live)
		}
	}
	if err := os.WriteFile(out, append(append(snapshot, '\n'), live...), 0o644); err != nil {
		t.Fatal(err)
	}
}
