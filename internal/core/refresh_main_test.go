package core

import (
	"testing"
	"time"
)

func TestNextAlignedRun(t *testing.T) {
	at := func(h, m int) time.Time { return time.Date(2026, 1, 2, h, m, 30, 0, time.UTC) }
	want := func(h, m int) time.Time { return time.Date(2026, 1, 2, h, m, 0, 0, time.UTC) }

	cases := []struct {
		now      time.Time
		interval int // seconds
		next     time.Time
	}{
		{at(10, 5), 1800, want(10, 30)},  // 30m: 10:05 -> 10:30
		{at(10, 45), 1800, want(11, 0)},  // 30m: 10:45 -> 11:00
		{at(10, 30), 1800, want(11, 0)},  // exactly on boundary+30s -> next
		{at(10, 12), 900, want(10, 15)},  // 15m -> :15
		{at(10, 0), 600, want(10, 10)},   // 10m
		{at(10, 50), 25 * 60, want(11, 0)}, // 25m: 50 -> next hour (cron wrap)
		{at(10, 20), 25 * 60, want(10, 25)}, // 25m: 20 -> 25
		{at(10, 5), 3600, want(11, 0)},   // 60m -> top of next hour
	}
	for _, c := range cases {
		got := nextAlignedRun(c.now, c.interval)
		if !got.Equal(c.next) {
			t.Errorf("nextAlignedRun(%s, %ds) = %s, want %s", c.now.Format("15:04:05"), c.interval, got.Format("15:04"), c.next.Format("15:04"))
		}
	}
}
