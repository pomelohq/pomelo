package lock

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
)

const lockDir = "/tmp/pom"

func AcquirePrimary(session string) (release func(), ok bool) {
	EnsureDir()
	f, err := os.OpenFile(lockPath(session, "primary"), os.O_CREATE|os.O_RDWR, 0o644)
	if err != nil {
		return func() {}, false
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		_ = f.Close()
		return func() {}, false
	}
	_ = f.Truncate(0)
	_, _ = f.WriteAt([]byte(fmt.Sprintf("%d\n", os.Getpid())), 0)
	return func() { _ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN); _ = f.Close() }, true
}

func TryAcquire(session, service string) (release func(), ok bool) {
	EnsureDir()
	f, err := os.OpenFile(lockPath(session, service), os.O_CREATE|os.O_RDWR, 0o644)
	if err != nil {
		return func() {}, false
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		_ = f.Close()
		return func() {}, false
	}
	return func() { _ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN); _ = f.Close() }, true
}

func lockPath(session, service string) string {
	return filepath.Join(lockDir, fmt.Sprintf("%s_%s.lock", session, service))
}

func EnsureDir() {
	_ = os.MkdirAll(lockDir, 0o755)
}

func Acquire(session, service string) {
	EnsureDir()
	_ = os.WriteFile(lockPath(session, service), []byte(fmt.Sprintf("%d", os.Getpid())), 0o644)
}

func Release(session, service string) {
	_ = os.Remove(lockPath(session, service))
}

func ReleaseAll(session string) {
	entries, err := os.ReadDir(lockDir)
	if err != nil {
		return
	}
	prefix := session + "_"
	for _, e := range entries {
		name := e.Name()
		if strings.HasPrefix(name, prefix) && strings.HasSuffix(name, ".lock") {
			_ = os.Remove(filepath.Join(lockDir, name))
		}
	}
}
