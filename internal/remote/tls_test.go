package remote

import (
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"io"
	"net/http"
	"testing"
)

func TestLoadOrCreateCertIsStable(t *testing.T) {
	dir := t.TempDir()
	c1, fp1, err := LoadOrCreateCert(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(fp1) != 64 {
		t.Errorf("fingerprint should be 32-byte hex, got %q", fp1)
	}
	// Second load reuses the same cert on disk, so the pinned fingerprint holds.
	c2, fp2, err := LoadOrCreateCert(dir)
	if err != nil {
		t.Fatal(err)
	}
	if fp1 != fp2 {
		t.Errorf("fingerprint changed across loads: %s vs %s", fp1, fp2)
	}
	sum := sha256.Sum256(c2.Certificate[0])
	if hex.EncodeToString(sum[:]) != fp1 {
		t.Error("fingerprint must be SHA-256 of the leaf DER")
	}
	_ = c1
}

func TestServesTLSAndPinMatches(t *testing.T) {
	dir := t.TempDir()
	cert, fp, err := LoadOrCreateCert(dir)
	if err != nil {
		t.Fatal(err)
	}
	srv := New(&fakeDisp{}, "secret", "9.9.9")
	srv.SetTLS(cert, fp)
	addr, err := srv.Start("127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer srv.Stop()

	// A pinning client trusts only the exact leaf fingerprint, not a CA.
	var seen string
	client := &http.Client{Transport: &http.Transport{
		TLSClientConfig: &tls.Config{
			InsecureSkipVerify: true, // pin manually below
			VerifyConnection: func(cs tls.ConnectionState) error {
				sum := sha256.Sum256(cs.PeerCertificates[0].Raw)
				seen = hex.EncodeToString(sum[:])
				return nil
			},
		},
	}}
	resp, err := client.Get("https://" + addr + "/ping")
	if err != nil {
		t.Fatalf("tls get: %v", err)
	}
	defer resp.Body.Close()
	_, _ = io.ReadAll(resp.Body)
	if resp.StatusCode != 200 {
		t.Errorf("ping status %d", resp.StatusCode)
	}
	if seen != fp {
		t.Errorf("served cert fingerprint %s != pinned %s", seen, fp)
	}
}
