package services

import (
	"fmt"
	"regexp"

	"github.com/pomelohq/pomelo/internal/config"
)

type EnvPair struct {
	Key    string `json:"key"`
	Value  string `json:"value"`
	Source string `json:"source"`
	Secret bool   `json:"secret"`
}

var secretTmplRe = regexp.MustCompile(`\{\{\s*secret\.`)

type ServiceExplain struct {
	Repo      string            `json:"repo"`
	Alias     string            `json:"alias"`
	Service   string            `json:"service"`
	Cmd       string            `json:"cmd"`
	Port      int               `json:"port"`
	Databases map[string]string `json:"databases"`
	Env       []EnvPair         `json:"env"`
}

func ExplainService(cfg *config.Config, repo, svcName, branch, envName string) (*ServiceExplain, error) {
	if cfg == nil {
		return nil, fmt.Errorf("no config")
	}
	dirName, dir := findRepoByNameOrAlias(cfg, repo)
	if dir == nil {
		return nil, fmt.Errorf("no repo %q", repo)
	}
	svc := dir.Services[svcName]
	if svc == nil {
		return nil, fmt.Errorf("no service %q in repo %q", svcName, dirName)
	}
	alias := dir.Alias
	if alias == "" {
		alias = dirName
	}
	branchSafe := BranchSafe(branch)
	wsKey := WsKey(branch, branch == cfg.GlobalDefaultBranch())

	dbNames := make(map[string]string, len(dir.Databases))
	for key, tpl := range dir.Databases {
		dbNames[key] = cfg.Session + "_" + ResolveBranchTokens(tpl, branch)
	}

	svcEnv := make(map[string]string)
	for k, v := range dir.Env {
		svcEnv[k] = v
	}
	for k, v := range svc.Env {
		svcEnv[k] = v
	}
	own := dir.OwnEnv()
	resolved := ResolveEnvTemplates(svcEnv, cfg, branchSafe, branch, wsKey, envName, dbNames)
	env := make([]EnvPair, len(resolved))
	for i, r := range resolved {
		env[i] = EnvPair{
			Key: r.Key, Value: r.Value, Source: envSource(cfg, dir, svc, own, r.Key),
			Secret: secretTmplRe.MatchString(svcEnv[r.Key]),
		}
	}

	return &ServiceExplain{
		Repo:      dirName,
		Alias:     alias,
		Service:   svcName,
		Cmd:       svc.ActiveCmd(""),
		Port:      findRepoServicePort(cfg, alias+"/"+svcName, wsKey),
		Databases: dbNames,
		Env:       env,
	}, nil
}

func envSource(cfg *config.Config, dir *config.Dir, svc *config.Service, own map[string]string, key string) string {
	if _, ok := own[key]; ok {
		return "own"
	}
	if svc != nil {
		if _, ok := svc.Env[key]; ok {
			return "own"
		}
	}
	for _, p := range dir.Presets_ {
		if preset := cfg.Presets[p]; preset != nil {
			if _, ok := preset.Env[key]; ok {
				return "preset:" + p
			}
		}
	}
	return ""
}

func findRepoByNameOrAlias(cfg *config.Config, repo string) (string, *config.Dir) {
	if d := cfg.Repos[repo]; d != nil {
		return repo, d
	}
	for _, n := range cfg.RepoOrder {
		if d := cfg.Repos[n]; d != nil && d.Alias == repo {
			return n, d
		}
	}
	return "", nil
}
