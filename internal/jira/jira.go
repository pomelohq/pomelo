package jira

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"regexp"
	"strings"
	"time"

	"github.com/pomelohq/pomelo/internal/appstate"
	"github.com/pomelohq/pomelo/internal/config"
	"github.com/pomelohq/pomelo/internal/secrets"
)

const DefaultTokenEnv = "JIRA_API_TOKEN"

const TokenSecret = "__jira_token"

var keyRe = regexp.MustCompile(`(?i)^[a-z][a-z0-9]*-\d+`)

func KeyForBranch(branch string) string {
	return strings.ToUpper(keyRe.FindString(branch))
}

type Client struct {
	Site  string
	Email string
	token string
}

var httpClient = &http.Client{Timeout: 8 * time.Second}

func Resolve(cfg *config.Config) *Client {
	site, email, tokenEnv := "", "", ""
	session := ""
	if cfg != nil {
		session = cfg.Session
	}
	if j := appstate.Load(session).Jira; j.Site != "" && j.Email != "" {
		site, email, tokenEnv = j.Site, j.Email, j.TokenEnv
	}
	if site == "" || email == "" {
		return nil
	}
	token, ok := secrets.Get(session, TokenSecret)
	if !ok || token == "" {
		if tokenEnv == "" {
			tokenEnv = DefaultTokenEnv
		}
		token = os.Getenv(tokenEnv)
	}
	if token == "" {
		return nil
	}
	return &Client{Site: normalizeSite(site), Email: email, token: token}
}

func normalizeSite(site string) string {
	site = strings.TrimRight(strings.TrimSpace(site), "/")
	if site != "" && !strings.HasPrefix(site, "http://") && !strings.HasPrefix(site, "https://") {
		site = "https://" + site
	}
	return site
}

func (c *Client) get(path string, out any) error {
	req, err := http.NewRequest(http.MethodGet, c.Site+path, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Authorization",
		"Basic "+base64.StdEncoding.EncodeToString([]byte(c.Email+":"+c.token)))
	resp, err := httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("jira: HTTP %d", resp.StatusCode)
	}
	return json.NewDecoder(resp.Body).Decode(out)
}

func (c *Client) BrowseURL(key string) string { return c.Site + "/browse/" + key }

func (c *Client) Host() string {
	if u, err := url.Parse(c.Site); err == nil {
		return u.Host
	}
	return ""
}

func (c *Client) FetchURL(rawurl string) ([]byte, string, error) {
	req, err := http.NewRequest(http.MethodGet, rawurl, nil)
	if err != nil {
		return nil, "", err
	}
	req.Header.Set("Authorization",
		"Basic "+base64.StdEncoding.EncodeToString([]byte(c.Email+":"+c.token)))
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, "", fmt.Errorf("jira: HTTP %d", resp.StatusCode)
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, 25<<20))
	if err != nil {
		return nil, "", err
	}
	return data, resp.Header.Get("Content-Type"), nil
}

type Issue struct {
	Key      string `json:"key"`
	Summary  string `json:"summary"`
	Status   string `json:"status"`
	Category string `json:"category"`
	Assignee string `json:"assignee"`
	URL      string `json:"url"`
}

func (c *Client) SearchByKeys(keys []string) ([]Issue, error) {
	if len(keys) == 0 {
		return nil, nil
	}
	jql := fmt.Sprintf("key in (%s)", strings.Join(keys, ","))
	var body struct {
		Issues []struct {
			Key    string `json:"key"`
			Fields struct {
				Summary string `json:"summary"`
				Status  struct {
					Name     string `json:"name"`
					Category struct {
						Key string `json:"key"`
					} `json:"statusCategory"`
				} `json:"status"`
				Assignee *struct {
					DisplayName string `json:"displayName"`
				} `json:"assignee"`
			} `json:"fields"`
		} `json:"issues"`
	}
	err := c.get("/rest/api/3/search/jql?fields=summary,status,assignee&maxResults=100&jql="+url.QueryEscape(jql), &body)
	if err != nil {
		return nil, err
	}
	out := make([]Issue, 0, len(body.Issues))
	for _, iss := range body.Issues {
		assignee := ""
		if iss.Fields.Assignee != nil {
			assignee = iss.Fields.Assignee.DisplayName
		}
		out = append(out, Issue{
			Key: iss.Key, Summary: iss.Fields.Summary,
			Status: iss.Fields.Status.Name, Category: iss.Fields.Status.Category.Key,
			Assignee: assignee, URL: c.BrowseURL(iss.Key),
		})
	}
	return out, nil
}

type Comment struct {
	Author  string `json:"author"`
	Created string `json:"created"`
	Body    string `json:"body"`
}

type IssueDetail struct {
	Key, Summary, Status, URL, Description string
	Comments                               []Comment `json:"comments"`
}

func (c *Client) IssueWithDescription(key string) (*IssueDetail, error) {
	var body struct {
		Fields struct {
			Summary     string                `json:"summary"`
			Status      struct{ Name string } `json:"status"`
			Description json.RawMessage       `json:"description"`
			Attachment  []struct {
				Filename string `json:"filename"`
				MimeType string `json:"mimeType"`
				Content  string `json:"content"`
			} `json:"attachment"`
			Comment struct {
				Comments []struct {
					Author  struct{ DisplayName string } `json:"author"`
					Created string                       `json:"created"`
					Body    json.RawMessage              `json:"body"`
				} `json:"comments"`
			} `json:"comment"`
		} `json:"fields"`
	}
	if err := c.get("/rest/api/3/issue/"+key+"?fields=summary,status,description,attachment,comment", &body); err != nil {
		return nil, err
	}
	desc := ADFMarkdown(body.Fields.Description)
	var imgs []string
	for _, a := range body.Fields.Attachment {
		if strings.HasPrefix(a.MimeType, "image/") && a.Content != "" {
			imgs = append(imgs, "!["+a.Filename+"]("+a.Content+")")
		}
	}
	if len(imgs) > 0 {
		desc = strings.TrimSpace(desc) + "\n\n### Attachments\n\n" + strings.Join(imgs, "\n\n")
	}
	var comments []Comment
	for _, cm := range body.Fields.Comment.Comments {
		comments = append(comments, Comment{
			Author: cm.Author.DisplayName, Created: cm.Created, Body: ADFMarkdown(cm.Body),
		})
	}
	return &IssueDetail{
		Key: key, Summary: body.Fields.Summary, Status: body.Fields.Status.Name,
		URL:         c.BrowseURL(key),
		Description: desc,
		Comments:    comments,
	}, nil
}

func ADFMarkdown(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var doc map[string]any
	if json.Unmarshal(raw, &doc) != nil {
		return ""
	}
	var blocks []string
	if content, ok := doc["content"].([]any); ok {
		for _, c := range content {
			if m, ok := c.(map[string]any); ok {
				blocks = append(blocks, adfBlock(m))
			}
		}
	}
	return strings.TrimSpace(strings.Join(blocks, "\n\n"))
}

func adfInline(nodes []any) string {
	var b strings.Builder
	for _, n := range nodes {
		m, ok := n.(map[string]any)
		if !ok {
			continue
		}
		switch m["type"] {
		case "hardBreak":
			b.WriteString("\n")
		case "mention":
			if a, ok := m["attrs"].(map[string]any); ok {
				if t, _ := a["text"].(string); t != "" {
					b.WriteString("**" + t + "**")
				}
			}
		case "emoji":
			if a, ok := m["attrs"].(map[string]any); ok {
				if t, _ := a["text"].(string); t != "" {
					b.WriteString(t)
				}
			}
		case "text":
			t, _ := m["text"].(string)
			href, code, strong, em := "", false, false, false
			if ms, ok := m["marks"].([]any); ok {
				for _, mk := range ms {
					mm, _ := mk.(map[string]any)
					switch mm["type"] {
					case "code":
						code = true
					case "strong":
						strong = true
					case "em":
						em = true
					case "link":
						if a, ok := mm["attrs"].(map[string]any); ok {
							href, _ = a["href"].(string)
						}
					}
				}
			}
			if code {
				t = "`" + t + "`"
			}
			if strong {
				t = "**" + t + "**"
			}
			if em {
				t = "_" + t + "_"
			}
			if href != "" {
				t = "[" + t + "](" + href + ")"
			}
			b.WriteString(t)
		}
	}
	return b.String()
}

func adfBlock(m map[string]any) string {
	content, _ := m["content"].([]any)
	switch m["type"] {
	case "heading":
		level := 2
		if a, ok := m["attrs"].(map[string]any); ok {
			if l, ok := a["level"].(float64); ok {
				level = int(l)
			}
		}
		return strings.Repeat("#", level) + " " + adfInline(content)
	case "bulletList":
		var out []string
		for _, li := range content {
			if lim, ok := li.(map[string]any); ok {
				out = append(out, "- "+adfListItem(lim))
			}
		}
		return strings.Join(out, "\n")
	case "taskList":
		var out []string
		for _, ti := range content {
			tim, ok := ti.(map[string]any)
			if !ok {
				continue
			}
			box := "[ ] "
			if a, ok := tim["attrs"].(map[string]any); ok {
				if s, _ := a["state"].(string); s == "DONE" {
					box = "[x] "
				}
			}
			kids, _ := tim["content"].([]any)
			out = append(out, "- "+box+adfInline(kids))
		}
		return strings.Join(out, "\n")
	case "orderedList":
		var out []string
		for i, li := range content {
			if lim, ok := li.(map[string]any); ok {
				out = append(out, fmt.Sprintf("%d. %s", i+1, adfListItem(lim)))
			}
		}
		return strings.Join(out, "\n")
	case "mediaSingle", "mediaGroup":
		var out []string
		for _, c := range content {
			cm, ok := c.(map[string]any)
			if !ok {
				continue
			}
			a, _ := cm["attrs"].(map[string]any)
			if a == nil {
				continue
			}
			if t, _ := a["type"].(string); t == "external" {
				if u, _ := a["url"].(string); u != "" {
					out = append(out, "!["+"]("+u+")")
				}
			}
		}
		return strings.Join(out, "\n\n")
	case "codeBlock":
		return "```\n" + adfInline(content) + "\n```"
	case "rule":
		return "---"
	case "blockquote":
		var out []string
		for _, c := range content {
			if cm, ok := c.(map[string]any); ok {
				out = append(out, "> "+adfBlock(cm))
			}
		}
		return strings.Join(out, "\n")
	default:
		return adfInline(content)
	}
}

func adfListItem(li map[string]any) string {
	content, _ := li["content"].([]any)
	var inline []string
	var nested []string
	for _, c := range content {
		cm, ok := c.(map[string]any)
		if !ok {
			continue
		}
		switch cm["type"] {
		case "bulletList", "orderedList", "taskList":
			nested = append(nested, indentLines(adfBlock(cm), "   "))
		default:
			inline = append(inline, adfBlock(cm))
		}
	}
	s := strings.Join(inline, " ")
	if len(nested) > 0 {
		s += "\n" + strings.Join(nested, "\n")
	}
	return s
}

func indentLines(s, pad string) string {
	lines := strings.Split(s, "\n")
	for i := range lines {
		lines[i] = pad + lines[i]
	}
	return strings.Join(lines, "\n")
}

type Board struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
	Type string `json:"type"`
}

func (c *Client) Boards() ([]Board, error) {
	var body struct {
		Values []Board `json:"values"`
	}
	if err := c.get("/rest/agile/1.0/board?maxResults=50", &body); err != nil {
		return nil, err
	}
	return body.Values, nil
}

type Sprint struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
}

func (c *Client) ActiveSprints(boardID int) ([]Sprint, error) {
	var body struct {
		Values []Sprint `json:"values"`
	}
	if err := c.get(fmt.Sprintf("/rest/agile/1.0/board/%d/sprint?state=active&maxResults=10", boardID), &body); err != nil {
		return nil, err
	}
	return body.Values, nil
}

type SprintIssue struct {
	Key       string `json:"key"`
	Summary   string `json:"summary"`
	Status    string `json:"status"`
	Assignee  string `json:"assignee"`
	Avatar    string `json:"avatar"`
	AccountID string `json:"account_id"`
	Sprint    string `json:"sprint"`
}

func (c *Client) sprintIssues(sprintID int) ([]SprintIssue, error) {
	var body struct {
		Issues []struct {
			Key    string `json:"key"`
			Fields struct {
				Summary  string                `json:"summary"`
				Status   struct{ Name string } `json:"status"`
				Assignee *struct {
					DisplayName string            `json:"displayName"`
					AccountID   string            `json:"accountId"`
					AvatarUrls  map[string]string `json:"avatarUrls"`
				} `json:"assignee"`
			} `json:"fields"`
		} `json:"issues"`
	}
	path := fmt.Sprintf("/rest/agile/1.0/sprint/%d/issue?maxResults=200&fields=summary,status,assignee", sprintID)
	if err := c.get(path, &body); err != nil {
		return nil, err
	}
	out := make([]SprintIssue, 0, len(body.Issues))
	for _, iss := range body.Issues {
		si := SprintIssue{Key: iss.Key, Summary: iss.Fields.Summary, Status: iss.Fields.Status.Name}
		if a := iss.Fields.Assignee; a != nil {
			si.Assignee = a.DisplayName
			si.AccountID = a.AccountID
			si.Avatar = a.AvatarUrls["24x24"]
		}
		out = append(out, si)
	}
	return out, nil
}

func (c *Client) CurrentSprintIssues(boardID int) ([]SprintIssue, error) {
	sprints, err := c.ActiveSprints(boardID)
	if err != nil {
		return nil, err
	}
	var out []SprintIssue
	for _, sp := range sprints {
		issues, err := c.sprintIssues(sp.ID)
		if err != nil {
			continue
		}
		for i := range issues {
			issues[i].Sprint = sp.Name
		}
		out = append(out, issues...)
	}
	return out, nil
}

func (c *Client) MyAccountID() (string, error) {
	var me struct {
		AccountID string `json:"accountId"`
	}
	if err := c.get("/rest/api/3/myself", &me); err != nil {
		return "", err
	}
	return me.AccountID, nil
}

func Myself(site, email, token string) (name, addr string, err error) {
	c := &Client{Site: strings.TrimRight(site, "/"), Email: email, token: token}
	var me struct {
		DisplayName  string `json:"displayName"`
		EmailAddress string `json:"emailAddress"`
	}
	if err := c.get("/rest/api/3/myself", &me); err != nil {
		return "", "", err
	}
	return me.DisplayName, me.EmailAddress, nil
}

func ADFText(raw json.RawMessage) string {
	if len(raw) == 0 {
		return ""
	}
	var node map[string]any
	if json.Unmarshal(raw, &node) != nil {
		return ""
	}
	var b strings.Builder
	var walk func(n map[string]any)
	walk = func(n map[string]any) {
		if t, ok := n["text"].(string); ok {
			b.WriteString(t)
		}
		if kids, ok := n["content"].([]any); ok {
			for _, k := range kids {
				if km, ok := k.(map[string]any); ok {
					walk(km)
				}
			}
		}
		switch n["type"] {
		case "paragraph", "heading", "listItem", "codeBlock":
			b.WriteString("\n")
		}
	}
	walk(node)
	return b.String()
}
