package core

func (s *Server) JiraBoards() map[string]any                  { return s.jira.Boards() }
func (s *Server) JiraSprint(board int) map[string]any         { return s.jira.Sprint(board) }
func (s *Server) JiraIssue(key string, force bool) map[string]any { return s.jira.Issue(key, force) }
func (s *Server) JiraIssues(branches []string) map[string]any { return s.jira.Issues(branches) }
func (s *Server) JiraTest(site, email, token string) map[string]any {
	return s.jira.Test(site, email, token)
}
