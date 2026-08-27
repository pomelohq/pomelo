//go:build !darwin

package ptyhost

func isZombie(int) bool { return false }
