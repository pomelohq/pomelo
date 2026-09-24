package secrets

import (
	"encoding/json"
	"os"
	"testing"
)

// TestOracleInterop proves the Rust port and this store read each other's files: it dumps the
// session's secrets as JSON, then writes one secret of its own.
func TestOracleInterop(t *testing.T) {
	state, session, out := os.Getenv("POM_ORACLE_STATE_HOME"), os.Getenv("POM_ORACLE_SESSION"), os.Getenv("POM_ORACLE_OUT")
	if state == "" || session == "" || out == "" {
		t.Skip("interop check only runs when driven by the parity tests")
	}
	t.Setenv("XDG_STATE_HOME", state)
	values := map[string]string{}
	for _, name := range Names(session) {
		v, _ := Get(session, name)
		values[name] = v
	}
	data, err := json.Marshal(values)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(out, data, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Set(session, "FROM_GO", "go value"); err != nil {
		t.Fatal(err)
	}
}
