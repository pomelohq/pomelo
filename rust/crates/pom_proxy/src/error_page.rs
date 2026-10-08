//! The page a browser gets instead of a bare error line: what is wrong with the service, what to do about it,
//! and a check that reloads the page once the service answers.

use crate::routing::{Problem, ProblemKind};

/// How often the page asks the proxy again, by what it is waiting for.
fn poll_ms(kind: ProblemKind) -> Option<u32> {
    match kind {
        ProblemKind::Starting => Some(1500),
        ProblemKind::Stopped | ProblemKind::Unreachable => Some(3000),
        ProblemKind::NoRoute => Some(5000),
        ProblemKind::NotFound => None,
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn shell_word(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./@".contains(c))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

pub fn render(status: u16, message: &str, problem: &Problem) -> String {
    let target = if problem.target.is_empty() {
        "This service".to_string()
    } else {
        problem.target.clone()
    };
    let in_workspace = if problem.workspace.is_empty() {
        String::new()
    } else {
        format!(" in <b>{}</b>", escape(&problem.workspace))
    };
    let start = (!problem.target.is_empty() && !problem.workspace.is_empty()).then(|| {
        format!(
            "pom -w {} start {}",
            shell_word(&problem.workspace),
            shell_word(&problem.target)
        )
    });
    let (tone, title, lead) = match problem.kind {
        ProblemKind::Starting => (
            "wait",
            format!("{} is starting", escape(&target)),
            format!(
                "It is building{in_workspace} and not listening yet. This page opens it as soon as it answers."
            ),
        ),
        ProblemKind::Stopped => (
            "stop",
            format!("{} is not running", escape(&target)),
            format!(
                "Start it{in_workspace} from the Services panel in Pomelo, or run the command below. This page opens it once it is up."
            ),
        ),
        ProblemKind::Unreachable => (
            "error",
            format!("{} does not answer", escape(&target)),
            format!(
                "Nothing took the connection{in_workspace}. It may have crashed or be restarting: check its tab in Pomelo. This page retries on its own."
            ),
        ),
        ProblemKind::NoRoute => (
            "error",
            format!("No route for {}", escape(&target)),
            format!(
                "No workspace or service matches this address{in_workspace}. Check the name, or create the workspace in Pomelo; this page retries on its own."
            ),
        ),
        ProblemKind::NotFound => (
            "error",
            "Not a workspace address".to_string(),
            "Service URLs look like <code>http://&lt;service&gt;.&lt;repo&gt;.&lt;workspace&gt;.localhost:&lt;port&gt;</code>."
                .to_string(),
        ),
    };
    let command = start
        .filter(|_| matches!(problem.kind, ProblemKind::Stopped | ProblemKind::Unreachable))
        .map(|command| {
            format!(
                r#"<div class="cmd"><code id="cmd">{}</code><button type="button" id="copy">Copy</button></div>"#,
                escape(&command)
            )
        })
        .unwrap_or_default();
    let poll = poll_ms(problem.kind);
    let status_line = match poll {
        Some(_) => {
            r#"<p class="watch"><span class="dot"></span><span id="watch">Checking again...</span></p>"#
        }
        None => "",
    };
    let script = poll
        .map(|every| {
            format!(
                r#"<script>
(function () {{
  var started = Date.now(), status = {status}, every = {every};
  var label = document.getElementById("watch");
  function tick() {{
    fetch(location.href, {{ method: "HEAD", cache: "no-store", redirect: "manual" }})
      .then(function (answer) {{
        if (answer.status !== status) {{ location.reload(); return; }}
        var seconds = Math.round((Date.now() - started) / 1000);
        if (label) label.textContent = "Still waiting - " + seconds + "s. Checking every " + every / 1000 + "s.";
        setTimeout(tick, every);
      }})
      .catch(function () {{ setTimeout(tick, every); }});
  }}
  setTimeout(tick, every);
  var copy = document.getElementById("copy");
  if (copy) copy.addEventListener("click", function () {{
    navigator.clipboard.writeText(document.getElementById("cmd").textContent).then(function () {{
      copy.textContent = "Copied";
      setTimeout(function () {{ copy.textContent = "Copy"; }}, 1500);
    }});
  }});
}})();
</script>
<noscript><meta http-equiv="refresh" content="{refresh}"></noscript>"#,
                refresh = every.div_ceil(1000).max(2),
            )
        })
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} - Pomelo</title>
<style>
:root {{
  --bg: #1f2228; --card: #282c33; --border: #3b414d; --text: #dce0e5; --muted: #a9afbc;
  --wait: #dec184; --stop: #74ade8; --error: #d07277; --code: #2f343e;
}}
@media (prefers-color-scheme: light) {{
  :root {{
    --bg: #f4f4f5; --card: #ffffff; --border: #dfe1e5; --text: #24292f; --muted: #5d6470;
    --wait: #9a6b00; --stop: #2f6fb8; --error: #b3404a; --code: #eef0f3;
  }}
}}
* {{ box-sizing: border-box; }}
body {{
  margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 24px 16px;
  background: var(--bg); color: var(--text);
  font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
}}
main {{
  width: 100%; max-width: 620px; background: var(--card); border: 1px solid var(--border);
  border-radius: 12px; padding: 28px 28px 22px;
}}
.badge {{
  display: inline-flex; align-items: center; gap: 8px; font: 600 12px/1 ui-monospace, SFMono-Regular, Menlo, monospace;
  letter-spacing: .04em; color: var(--accent); text-transform: uppercase;
}}
.badge::before {{ content: ""; width: 8px; height: 8px; border-radius: 50%; background: var(--accent); }}
.wait {{ --accent: var(--wait); }} .stop {{ --accent: var(--stop); }} .error {{ --accent: var(--error); }}
.wait .badge::before {{ animation: pulse 1.2s ease-in-out infinite; }}
h1 {{ font-size: 22px; line-height: 1.3; margin: 12px 0 8px; font-weight: 600; word-break: break-word; }}
p {{ margin: 0 0 14px; color: var(--muted); }}
p b {{ color: var(--text); font-weight: 600; }}
code {{ font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }}
p code {{ background: var(--code); padding: 1px 6px; border-radius: 4px; color: var(--text); }}
.cmd {{
  display: flex; align-items: center; gap: 10px; background: var(--code); border: 1px solid var(--border);
  border-radius: 8px; padding: 10px 10px 10px 14px; margin: 4px 0 16px;
}}
.cmd code {{ flex: 1; overflow-x: auto; white-space: nowrap; }}
button {{
  font: 600 12px/1 -apple-system, system-ui, sans-serif; color: var(--text); background: transparent;
  border: 1px solid var(--border); border-radius: 6px; padding: 7px 10px; cursor: pointer;
}}
button:hover {{ background: var(--border); }}
.watch {{ display: flex; align-items: center; gap: 8px; font-size: 13px; }}
.dot {{ width: 7px; height: 7px; border-radius: 50%; background: var(--accent); animation: pulse 1.2s ease-in-out infinite; }}
details {{ border-top: 1px solid var(--border); margin-top: 6px; padding-top: 12px; color: var(--muted); font-size: 13px; }}
summary {{ cursor: pointer; }}
details code {{ display: block; margin-top: 8px; white-space: pre-wrap; word-break: break-word; }}
@keyframes pulse {{ 50% {{ opacity: .35; }} }}
@media (prefers-reduced-motion: reduce) {{ .dot, .badge::before {{ animation: none !important; }} }}
</style>
</head>
<body>
<main class="{tone}">
<span class="badge">{status} - Pomelo dev proxy</span>
<h1>{title}</h1>
<p>{lead}</p>
{command}
{status_line}
<details><summary>Details</summary><code>{message}</code></details>
</main>
{script}
</body>
</html>
"#,
        message = escape(message),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(kind: ProblemKind) -> Problem {
        Problem {
            kind,
            target: "api/server".into(),
            workspace: "feat-login".into(),
        }
    }

    #[test]
    fn a_starting_service_gets_a_page_that_reloads_when_it_answers() {
        let page = render(
            503,
            "dev-proxy: api/server is still starting",
            &problem(ProblemKind::Starting),
        );
        assert!(page.contains("api/server is starting"), "{page}");
        assert!(page.contains("status = 503") && page.contains("every = 1500"));
        assert!(page.contains("location.reload()"));
        assert!(
            !page.contains("id=\"cmd\""),
            "no start command while it starts"
        );
    }

    #[test]
    fn a_stopped_service_shows_how_to_start_it() {
        let page = render(503, "stopped", &problem(ProblemKind::Stopped));
        assert!(
            page.contains("pom -w feat-login start api/server"),
            "{page}"
        );
        assert!(page.contains("every = 3000"));
    }

    #[test]
    fn a_wrong_address_does_not_poll_and_text_is_escaped() {
        let page = render(404, "no route for <script>", &Problem::default());
        assert!(!page.contains("location.reload()"));
        assert!(page.contains("no route for &lt;script&gt;"));
        assert!(!page.contains("no route for <script>"));
    }

    #[test]
    fn odd_names_are_quoted_in_the_command() {
        let page = render(
            502,
            "x",
            &Problem {
                kind: ProblemKind::Unreachable,
                target: "api/server".into(),
                workspace: "feat x".into(),
            },
        );
        assert!(
            page.contains("pom -w &#39;feat x&#39; start api/server"),
            "{page}"
        );
    }
}
