//go:build darwin

package ptyhost

import "golang.org/x/sys/unix"

const sZomb = 5 // SZOMB in <sys/proc.h> (SIDL 1, SRUN 2, SSLEEP 3, SSTOP 4, SZOMB 5)

// isZombie reports whether pid is an exited-but-unreaped process. kill(pid, 0)
// succeeds for zombies, so callers need this to tell a dead holder from a live one.
func isZombie(pid int) bool {
	kp, err := unix.SysctlKinfoProc("kern.proc.pid", pid)
	return err == nil && kp.Proc.P_stat == sZomb
}
