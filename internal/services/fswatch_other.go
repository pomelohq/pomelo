//go:build !darwin || !cgo

package services

import "errors"

func WatchTree(root string, onChange func()) (func(), error) {
	return func() {}, errors.New("fswatch: unsupported on this platform")
}
