package tracker

type Provider interface {
	Name() string
	Boards() map[string]any
	Sprint(board int) map[string]any
	Issue(key string, force bool) map[string]any
	Issues(branches []string) map[string]any
	Test(site, email, token string) map[string]any
}
