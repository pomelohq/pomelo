package core

import (
	"net/http"

	"github.com/pomelohq/pomelo/internal/plugin"
)

type versionFeature struct{}

func (*versionFeature) Name() string { return "version" }

func (s *versionFeature) Routes(mux *http.ServeMux) {
	mux.HandleFunc("/api/version", s.handleVersion)
}

func (s *versionFeature) handleVersion(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, VersionInfo())
}

func VersionInfo() map[string]any {
	return map[string]any{
		"version":      Version,
		"releases_url": "https://github.com/pomelohq/pomelo/releases/latest",
	}
}

var _ plugin.HTTPProvider = (*versionFeature)(nil)
