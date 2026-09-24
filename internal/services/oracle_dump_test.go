package services

import (
	"encoding/json"
	"os"
	"testing"

	"github.com/pomelohq/pomelo/internal/config"
)

type oracleContext struct {
	Branch  string            `json:"branch"`
	WsKey   string            `json:"ws_key"`
	EnvName string            `json:"env_name"`
	DBNames map[string]string `json:"db_names"`
	Inputs  []string          `json:"inputs"`
}

type oracleCases struct {
	Config      string          `json:"config"`
	Contexts    []oracleContext `json:"contexts"`
	Branches    []string        `json:"branches"`
	SharedPorts [][2]string     `json:"shared_ports"`
}

// TestOracleResolve writes resolver and branch-helper outputs for the Rust port's parity tests.
// State is the empty temp dir from TestMain, so only stable (hash) shared ports are in play.
func TestOracleResolve(t *testing.T) {
	in, out := os.Getenv("POM_ORACLE_CASES"), os.Getenv("POM_ORACLE_OUT")
	if in == "" || out == "" {
		t.Skip("oracle dump only runs when driven by the parity tests")
	}
	data, err := os.ReadFile(in)
	if err != nil {
		t.Fatal(err)
	}
	var cases oracleCases
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	cfg, err := config.Load(cases.Config)
	if err != nil {
		t.Fatal(err)
	}
	SetSharedStable(cfg.Session)
	defer SetSharedStable("")

	var resolved [][]string
	for _, c := range cases.Contexts {
		ctx := ResolveCtx{Cfg: cfg, Branch: c.Branch, WsKey: c.WsKey, EnvName: c.EnvName, DBNames: c.DBNames}
		var row []string
		for _, s := range c.Inputs {
			row = append(row, ResolveTokens(s, ctx))
		}
		resolved = append(resolved, row)
	}
	var branches []map[string]string
	for _, b := range cases.Branches {
		branches = append(branches, map[string]string{
			"safe": BranchSafe(b), "hash": BranchHash(b), "host": BranchHost(b),
			"label": WorkspaceLabel(b), "port_ws_key": PortWsKey(b),
		})
	}
	var ports []int
	for _, p := range cases.SharedPorts {
		ports = append(ports, StableSharedPort(p[0], p[1]))
	}
	result, err := json.Marshal(map[string]any{"resolved": resolved, "branches": branches, "stable_ports": ports})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(out, result, 0o644); err != nil {
		t.Fatal(err)
	}
}
