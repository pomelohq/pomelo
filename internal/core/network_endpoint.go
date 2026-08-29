package core

import (
	"net/http"
	"strconv"
	"time"

	"github.com/pomelohq/pomelo/internal/services"
)

func (s *Server) handleNetwork(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.NetworkInfo())
}

func (s *Server) NetworkInfo() map[string]any {
	cfg := s.cfg()
	if cfg == nil {
		return map[string]any{"error": "no project"}
	}
	const domain = "localhost"
	proxyPort := s.webPort() + 2
	whPort := s.webPort() + 1
	return map[string]any{
		"bind_ip":         services.BindIP(),
		"domain":          domain,
		"proxy_port":      proxyPort,
		"proxy_url":       "http://<service>.<repo>.<branch>." + domain + ":" + strconv.Itoa(proxyPort),
		"proxy_running":   s.devProxyListening(),
		"webhook":         map[string]any{"configured": true, "enabled": true, "listen_port": whPort},
		"webhook_running": s.webhookListening(),
	}
}

func (s *Server) webhookListening() bool {
	return controlPortListening(s.webPort() + 1)
}

func (s *Server) NetworkStart() map[string]any {
	if !s.devProxyListening() {
		s.startDevProxy()
	}
	if !s.webhookListening() {
		go s.startWebhookRelay()
		time.Sleep(150 * time.Millisecond)
	}
	return s.NetworkInfo()
}

func (s *Server) NetworkSetPorts(proxyPort, webhookPort int) map[string]any {
	return map[string]any{"ok": true}
}
