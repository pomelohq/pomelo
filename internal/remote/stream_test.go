package remote

import (
	"bufio"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/pomelohq/pomelo/internal/stream"
)

type fakeInput struct{}

func (fakeInput) Send(string) {}
func (fakeInput) Stop()       {}

type fakeStreamer struct{ sent []string }

func (f *fakeStreamer) OpenAgentStream(sink stream.Sink, done <-chan struct{}, branch string, isMain bool, mode, model, role string) AgentInput {
	go func() {
		_ = sink.SendJSONBytes([]byte(`{"type":"text","v":"hi ` + branch + `"}`))
		<-done
	}()
	return fakeInput{}
}
func (f *fakeStreamer) AgentSend(branch string, isMain bool, mode, model, text string) {
	f.sent = append(f.sent, text)
}

func TestAgentSendForwardsAndValidates(t *testing.T) {
	fs := &fakeStreamer{}
	srv := New(&fakeDisp{}, "secret", "0")
	srv.SetStreamer(fs)
	h := srv.Handler()

	if r := post(t, h, "/rpc/agent/send", "secret", `{"branch":"feat","text":"go on"}`); r.StatusCode != 200 {
		t.Fatalf("send status %d", r.StatusCode)
	}
	if len(fs.sent) != 1 || fs.sent[0] != "go on" {
		t.Errorf("nudge should forward text: %v", fs.sent)
	}
	if r := post(t, h, "/rpc/agent/send", "secret", `{"branch":"feat","text":""}`); r.StatusCode != 400 {
		t.Errorf("empty text: want 400, got %d", r.StatusCode)
	}
	if r := post(t, h, "/rpc/agent/send", "", `{"branch":"feat","text":"x"}`); r.StatusCode != 401 {
		t.Errorf("no token: want 401, got %d", r.StatusCode)
	}
}

func TestAgentStreamAuthMethod(t *testing.T) {
	srv := New(&fakeDisp{}, "secret", "0")
	srv.SetStreamer(&fakeStreamer{})
	h := srv.Handler()

	// POST to the stream endpoint is rejected (it is GET SSE).
	if r := post(t, h, "/rpc/stream/agent", "secret", ""); r.StatusCode != http.StatusMethodNotAllowed {
		t.Errorf("POST stream: want 405, got %d", r.StatusCode)
	}
	// GET without a token is unauthorized.
	req := httptest.NewRequest(http.MethodGet, "/rpc/stream/agent", nil)
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Errorf("GET no token: want 401, got %d", rec.Code)
	}
}

func TestAgentStreamSSE(t *testing.T) {
	srv := New(&fakeDisp{}, "secret", "0")
	srv.SetStreamer(&fakeStreamer{})
	ts := httptest.NewServer(srv.Handler())
	defer ts.Close()

	resp, err := http.Get(ts.URL + "/rpc/stream/agent?token=secret&branch=feat")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if ct := resp.Header.Get("Content-Type"); !strings.HasPrefix(ct, "text/event-stream") {
		t.Errorf("content-type: %q", ct)
	}

	line := make(chan string, 1)
	go func() {
		sc := bufio.NewScanner(resp.Body)
		for sc.Scan() {
			if t := sc.Text(); strings.HasPrefix(t, "data:") {
				line <- t
				return
			}
		}
	}()

	select {
	case got := <-line:
		if !strings.Contains(got, "hi feat") {
			t.Errorf("first SSE frame: %q", got)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("timed out waiting for an SSE frame")
	}
}
