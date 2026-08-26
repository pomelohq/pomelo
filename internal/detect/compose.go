package detect

import (
	_ "embed"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

//go:embed catalog.yaml
var catalogYAML []byte

// ServiceKind classifies a docker-compose service.
type ServiceKind string

const (
	KindApp     ServiceKind = "app"     // build: your code — runs native, not a shared service
	KindShared  ServiceKind = "shared"  // known backing image — a shared_service
	KindProxy   ServiceKind = "proxy"   // reverse proxy — Pomelo's dev-proxy replaces it
	KindUnknown ServiceKind = "unknown" // no image, no build
)

// ComposeService is one classified service from a docker-compose file.
type ComposeService struct {
	Name     string
	Kind     ServiceKind
	Image    string
	Type     string   // catalog type for a shared service ("custom" if unknown image)
	Strategy Strategy // isolation strategy for a shared service ("" if custom/unresolved)
	Clone    string   // per-branch seed method: template | dump-restore | none
	Ports    []int
	HasBuild bool
}

type catalogEntry struct {
	Type       string   `yaml:"type"`
	MatchImage []string `yaml:"match_image"`
	Strategy   Strategy `yaml:"strategy"`
	Ports      []int    `yaml:"ports"`
	Clone      string   `yaml:"clone"`
}

var proxyImages = []string{"nginx", "traefik", "caddy", "haproxy", "envoy"}

// Bare (un-namespaced) images that are backing services / dev tools, not the
// user's app — so a bare unknown image isn't blindly treated as a built app.
var bareBacking = []string{
	"adminer", "pgadmin", "phpmyadmin", "zookeeper", "consul", "vault", "etcd",
	"mosquitto", "eclipse-mosquitto", "prometheus", "grafana",
}

// ParseCompose finds a repo's docker-compose file and returns its classified
// services (backing → shared_service, build → app, reverse proxy → flagged).
func ParseCompose(repoPath string) []ComposeService {
	path := findComposeFile(repoPath)
	if path == "" {
		return nil
	}
	doc, err := readComposeFile(path)
	if err != nil {
		return nil
	}
	catalog := loadCatalog()
	dir := filepath.Dir(path)
	var out []ComposeService
	for _, name := range sortedKeys(doc.Services) {
		svc := doc.Services[name]
		image, hasBuild := resolveImage(svc, doc, dir, 0)
		cs := ComposeService{Name: name, Image: image, HasBuild: hasBuild}
		classify(&cs, catalog)
		out = append(out, cs)
	}
	return out
}

func classify(cs *ComposeService, catalog []catalogEntry) {
	if cs.HasBuild {
		cs.Kind = KindApp
		return
	}
	if cs.Image == "" {
		cs.Kind = KindUnknown
		return
	}
	img := strings.ToLower(cs.Image)
	for _, p := range proxyImages {
		if strings.Contains(img, p) {
			cs.Kind = KindProxy
			return
		}
	}
	for _, e := range catalog {
		for _, m := range e.MatchImage {
			if strings.Contains(img, strings.ToLower(m)) {
				cs.Kind = KindShared
				cs.Type = e.Type
				cs.Strategy = e.Strategy
				cs.Clone = e.Clone
				cs.Ports = e.Ports
				return
			}
		}
	}
	// Unknown image. A bare, un-namespaced name (e.g. "backend:latest") is almost
	// always a locally-built app image → treat as an app; a namespaced image
	// (acme/thing) is an unknown backing service the agent should classify.
	name := img
	if i := strings.IndexByte(name, ':'); i >= 0 {
		name = name[:i]
	}
	for _, b := range bareBacking {
		if name == b {
			cs.Kind = KindShared
			cs.Type = "custom"
			return
		}
	}
	if !strings.Contains(name, "/") {
		cs.Kind = KindApp
		return
	}
	cs.Kind = KindShared
	cs.Type = "custom"
}

type composeDoc struct {
	Services map[string]composeSvc `yaml:"services"`
}

type composeSvc struct {
	Image   string    `yaml:"image"`
	Build   yaml.Node `yaml:"build"`
	Extends yaml.Node `yaml:"extends"`
}

// resolveImage follows a service's `extends` chain (same-file, then file-based)
// to recover an image inherited from a base service.
func resolveImage(svc composeSvc, doc composeDoc, dir string, depth int) (string, bool) {
	if svc.Image != "" {
		return svc.Image, false
	}
	if svc.Build.Kind != 0 {
		return "", true
	}
	if depth > 6 || svc.Extends.Kind == 0 {
		return "", false
	}
	base, file := parseExtends(svc.Extends)
	if base == "" {
		return "", false
	}
	if file == "" {
		if b, ok := doc.Services[base]; ok {
			return resolveImage(b, doc, dir, depth+1)
		}
		return "", false
	}
	sub, err := readComposeFile(filepath.Join(dir, file))
	if err != nil {
		return "", false
	}
	if b, ok := sub.Services[base]; ok {
		return resolveImage(b, sub, filepath.Dir(filepath.Join(dir, file)), depth+1)
	}
	return "", false
}

func parseExtends(n yaml.Node) (service, file string) {
	switch n.Kind {
	case yaml.ScalarNode:
		return n.Value, ""
	case yaml.MappingNode:
		var m struct {
			Service string `yaml:"service"`
			File    string `yaml:"file"`
		}
		_ = n.Decode(&m)
		return m.Service, m.File
	}
	return "", ""
}

func findComposeFile(repoPath string) string {
	for _, name := range []string{"compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"} {
		p := filepath.Join(repoPath, name)
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	return ""
}

func readComposeFile(path string) (composeDoc, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return composeDoc{}, err
	}
	var doc composeDoc
	if err := yaml.Unmarshal(data, &doc); err != nil {
		return composeDoc{}, err
	}
	return doc, nil
}

func loadCatalog() []catalogEntry {
	var c []catalogEntry
	_ = yaml.Unmarshal(catalogYAML, &c)
	return c
}

func sortedKeys(m map[string]composeSvc) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
