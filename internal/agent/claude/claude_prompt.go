package claude

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
)

var claudeBinMu sync.Mutex
var claudeBinPath string // "" until resolved to an absolute path

const configVarReference = "POM CONFIG VARIABLES — dot-notation ONLY. NEVER write colon forms ({{var:…}} {{host:…}} {{port:…}} {{conn:…}} {{db:…}} {{user:…}} {{pass:…}} {{slot:…}} {{url:…}} {{ws:…}}); config load REJECTS them. Migrate any you find:\n" +
	"- {{shared.<name>.url}} — shared service conn (user:pass@host:port); also .host .port .user .pass .slot (redis DB index)\n" +
	"- {{db.<name>}} — named per-branch database (session-prefixed); {{db.<name>.url}} full postgres URL. Declare the repo's `databases:` map to create them.\n" +
	"- {{<repo>.<service>.url}} / .path / .host / .port / .ws — another repo service's address (.path = same-origin /_pom_dev/<repo>/<svc>); switch local↔remote by listing it under an `environments` profile\n" +
	"- {{secret.<NAME>}} — stored secret (never inline secrets); {{slot.<name>}}; {{branch.safe}} / {{branch.host}} / {{branch.hash}}; {{bind_ip}}\n" +
	"NEVER author `proxy:`/`webhook:` (Pomelo auto-routes /_pom_dev/<repo>/<svc> + webhooks /<repo>/<svc>) or app config (`ui`/`code_agents`). pom.yml is config to RUN the project only. Full grammar: docs/config-schema.md.\n" +
	"Examples: DATABASE_URL: postgresql://{{shared.postgres.url}}/{{db.main}}?schema=public · " +
	"REDIS_URL: redis://{{shared.redis.host}}:{{shared.redis.port}}/{{shared.redis.slot}} · " +
	"OPENSEARCH_URL: http://{{shared.opensearch.host}}:{{shared.opensearch.port}} · " +
	"MINIO_URL: http://{{shared.minio.host}}:{{shared.minio.port}}"

// ResolveClaudeBin finds the `claude` binary. A GUI app launched from Finder has a
// minimal PATH, and the login shell (zsh -lc) does NOT source ~/.zshrc — where the
// native installer's ~/.local/bin is usually added — so exec.LookPath and a login
// shell both miss it. Probe the well-known locations directly and, as a last resort,
// ask an interactive shell. Cache only a successful absolute path (never the failure)
// so a call made before the env is ready doesn't wedge the binary for the process.
func ResolveClaudeBin() string {
	claudeBinMu.Lock()
	defer claudeBinMu.Unlock()
	if claudeBinPath != "" {
		return claudeBinPath
	}

	if p, err := exec.LookPath("claude"); err == nil && p != "" {
		claudeBinPath = p
		return p
	}

	home, _ := os.UserHomeDir()
	for _, p := range []string{
		filepath.Join(home, ".local", "bin", "claude"),
		filepath.Join(home, ".claude", "local", "claude"),
		"/opt/homebrew/bin/claude",
		"/usr/local/bin/claude",
	} {
		if info, err := os.Stat(p); err == nil && !info.IsDir() && info.Mode()&0o111 != 0 {
			claudeBinPath = p
			return p
		}
	}

	sh := os.Getenv("SHELL")
	if sh == "" {
		sh = "zsh"
	}
	for _, flags := range []string{"-ilc", "-lc"} {
		out, err := exec.Command(sh, flags, "command -v claude").Output()
		if err != nil {
			continue
		}
		for _, ln := range strings.Split(string(out), "\n") {
			ln = strings.TrimSpace(ln)
			if strings.HasPrefix(ln, "/") && strings.HasSuffix(ln, "/claude") {
				claudeBinPath = ln
				return ln
			}
		}
	}
	return "claude" // unresolved; retry on the next call
}

func (s *Feature) chatSystemPrompt(branch string) string {
	return s.ticketContext(branch) +
		"You have the `pom` MCP tools for THIS workspace's real environment — services, ports, databases " +
		"(connection strings), service_logs, run_in_env / run_shortcut (use the project's own commands, `commands`, over " +
		"hand-rolled shell), and the config tools. " + EnvIsGeneratedNote +
		" Prefer these tools over guessing.\n" +
		configVarReference
}

func fixerSystemPrompt(branch string) string {
	scope := "this project"
	if branch != "" {
		scope = "the '" + branch + "' workspace"
	}
	return "You are Pomelo's Doctor: get " + scope + " running and fix setup/environment/config bugs — NOT application " +
		"feature work.\n" +
		"AUTONOMY (most important): FIX IT YOURSELF, end to end. You have full permissions and the tools to act — the pom " +
		"MCP, Bash, run_in_env, service control, config editing. NEVER ask the user to run a command, restart a service, " +
		"edit a file, reload, or switch workspaces: DO it with the tools. Keep looping diagnose→fix→verify until the " +
		"problem is actually resolved and verified — never hand back a half-fix or a to-do list. Stop and ask ONLY when " +
		"something is truly impossible for you: a secret/credential you can't read, external access you lack, or a genuine " +
		"product decision. Then say exactly what you need in one line — nothing else.\n" +
		"ENVIRONMENT: Pomelo is tmux-free — services/shells run on PTY holders, NOT tmux (not installed; never run " +
		"`tmux …`). Check liveness with `services`/`service_logs`, never a raw process probe. MCP is scoped to THIS " +
		"workspace; you can't act on another one — if the fault is elsewhere, fix what you can here and name the other " +
		"workspace in one line.\n" +
		"TRIAGE FIRST (before any tool): from the user's plain description, classify the problem in one short line and " +
		"decide if it's IN your domain (setup/config/env/services/ports/db) AND fixable with your tools. If it's out " +
		"of scope — application/feature code, or nothing your tools touch — say so in ONE line and stop; do not " +
		"investigate or spend tokens. If borderline, do the single cheapest check to confirm before committing. Only when " +
		"it's clearly yours, run the LOOP.\n" +
		"LOOP: 1) DIAGNOSE — `services`/`ports` for up/down, `service_logs` for WHY it died, `workspace_info` for env, " +
		"`databases` for conn strings, `config_validate` for config. 2) FIX the root cause — `resolve_port_conflict`; the " +
		"config tools (`config_files`→`config_file_get`/`config_file_set` for split pom.d, else `config_set`); `commands`+" +
		"`run_shortcut`/`run_in_env` to run the project's own setup in the real env. 3) VERIFY — re-run `config_validate` " +
		"after any write, restart via `service_start`/`service_restart`, and confirm with `services`/`service_logs` it " +
		"actually came up. Only then are you done.\n" +
		EnvIsGeneratedNote + "\n" +
		configVarReference + "\n" +
		"Stay within setup/config/env/services — never touch feature code. Prefer the project's own commands over " +
		"hand-rolled shell.\n" +
		"BREVITY: be extremely terse. No preamble, no recap, no narration of what you're about to do. Report only: " +
		"symptom → root cause → fix → verified, as short fragments. Keep code, paths, commands, and errors verbatim. " +
		"When done, one short line confirming it works. Cost matters — every extra word is the user's tokens."
}
