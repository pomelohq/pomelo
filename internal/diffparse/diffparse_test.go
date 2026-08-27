package diffparse

import "testing"

func TestPureRenameKeepsBothPaths(t *testing.T) {
	d := "diff --git a/a/old.ts b/a/new.ts\n" +
		"similarity index 100%\n" +
		"rename from a/old.ts\n" +
		"rename to a/new.ts"
	f := Parse(d)
	if len(f) != 1 {
		t.Fatalf("want 1 file, got %d", len(f))
	}
	if f[0].Status != "R" || f[0].Path != "a/new.ts" {
		t.Fatalf("status/path: %q %q", f[0].Status, f[0].Path)
	}
	if f[0].OldPath == nil || *f[0].OldPath != "a/old.ts" {
		t.Fatalf("oldPath: %v", f[0].OldPath)
	}
	if len(f[0].Lines) != 0 {
		t.Fatalf("pure rename has no content lines, got %d", len(f[0].Lines))
	}
}

func TestRenameWithEditsCountsHunks(t *testing.T) {
	d := "diff --git a/src/old.ts b/src/new.ts\n" +
		"similarity index 84%\n" +
		"rename from src/old.ts\n" +
		"rename to src/new.ts\n" +
		"index e4aade4..b4d15c1 100644\n" +
		"--- a/src/old.ts\n" +
		"+++ b/src/new.ts\n" +
		"@@ -1,2 +1,3 @@\n" +
		" hello\n" +
		" world\n" +
		"+extra"
	f := Parse(d)
	if f[0].Status != "R" || f[0].Path != "src/new.ts" {
		t.Fatalf("status/path: %q %q", f[0].Status, f[0].Path)
	}
	if f[0].OldPath == nil || *f[0].OldPath != "src/old.ts" {
		t.Fatalf("oldPath: %v", f[0].OldPath)
	}
	if f[0].Adds != 1 {
		t.Fatalf("adds: %d", f[0].Adds)
	}
}

func TestHeaderPathsWithSpaces(t *testing.T) {
	old, new := gitHeaderPaths("a/a/with space.ts b/a/final.ts")
	if old != "a/with space.ts" || new != "a/final.ts" {
		t.Fatalf("got %q %q", old, new)
	}
}

func TestDeletedFileWithSpaceKeepsPath(t *testing.T) {
	d := "diff --git a/a/gone file.ts b/a/gone file.ts\n" +
		"deleted file mode 100644\n" +
		"index 3b18e51..0000000\n" +
		"--- a/a/gone file.ts\n" +
		"+++ /dev/null\n" +
		"@@ -1 +0,0 @@\n" +
		"-hello world"
	f := Parse(d)
	if f[0].Status != "D" || f[0].Path != "a/gone file.ts" {
		t.Fatalf("status/path: %q %q", f[0].Status, f[0].Path)
	}
	if f[0].Dels != 1 {
		t.Fatalf("dels: %d", f[0].Dels)
	}
}

func TestPlainModificationIsUnaffected(t *testing.T) {
	d := "diff --git a/x.ts b/x.ts\n" +
		"index 111..222 100644\n" +
		"--- a/x.ts\n" +
		"+++ b/x.ts\n" +
		"@@ -1,1 +1,1 @@\n" +
		"-old\n" +
		"+new"
	f := Parse(d)
	if f[0].Status != "M" || f[0].Path != "x.ts" {
		t.Fatalf("status/path: %q %q", f[0].Status, f[0].Path)
	}
	if f[0].OldPath != nil {
		t.Fatalf("oldPath should be nil, got %v", *f[0].OldPath)
	}
	if f[0].Adds != 1 || f[0].Dels != 1 {
		t.Fatalf("adds/dels: %d %d", f[0].Adds, f[0].Dels)
	}
}

func TestLineNumbersTrackHunk(t *testing.T) {
	d := "diff --git a/x.ts b/x.ts\n" +
		"--- a/x.ts\n" +
		"+++ b/x.ts\n" +
		"@@ -10,3 +20,4 @@\n" +
		" ctx\n" +
		"-gone\n" +
		"+added\n" +
		"+more"
	f := Parse(d)
	want := []struct {
		kind Kind
		old  int
		new  int
	}{
		{KindHunk, -1, -1},
		{KindContext, 10, 20},
		{KindDel, 11, -1},
		{KindAdd, -1, 21},
		{KindAdd, -1, 22},
	}
	if len(f[0].Lines) != len(want) {
		t.Fatalf("want %d lines, got %d", len(want), len(f[0].Lines))
	}
	for i, w := range want {
		l := f[0].Lines[i]
		if l.Kind != w.kind {
			t.Fatalf("line %d kind %q want %q", i, l.Kind, w.kind)
		}
		if w.old >= 0 && (l.OldN == nil || *l.OldN != w.old) {
			t.Fatalf("line %d oldN %v want %d", i, l.OldN, w.old)
		}
		if w.new >= 0 && (l.NewN == nil || *l.NewN != w.new) {
			t.Fatalf("line %d newN %v want %d", i, l.NewN, w.new)
		}
	}
}
