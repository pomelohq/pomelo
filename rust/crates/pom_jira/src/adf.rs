//! Jira's document format (ADF) as markdown: headings, lists, task lists, code, quotes, rules, links, marks,
//! mentions and external images; anything else keeps its text.

use serde_json::Value;

pub fn adf_markdown(document: &Value) -> String {
    let blocks: Vec<String> = document["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(block)
        .collect();
    blocks.join("\n\n").trim().to_string()
}

fn children(node: &Value) -> &[Value] {
    node["content"].as_array().map_or(&[], Vec::as_slice)
}

fn inline(nodes: &[Value]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node["type"].as_str() {
            Some("hardBreak") => out.push('\n'),
            Some("mention") => {
                if let Some(name) = node["attrs"]["text"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                {
                    out.push_str(&format!("**{name}**"));
                }
            }
            Some("emoji") => out.push_str(node["attrs"]["text"].as_str().unwrap_or_default()),
            Some("text") => out.push_str(&marked(node)),
            _ => {}
        }
    }
    out
}

fn marked(node: &Value) -> String {
    let mut text = node["text"].as_str().unwrap_or_default().to_string();
    let (mut code, mut strong, mut emphasis, mut href) = (false, false, false, None);
    for mark in node["marks"].as_array().into_iter().flatten() {
        match mark["type"].as_str() {
            Some("code") => code = true,
            Some("strong") => strong = true,
            Some("em") => emphasis = true,
            Some("link") => href = mark["attrs"]["href"].as_str().map(str::to_string),
            _ => {}
        }
    }
    if code {
        text = format!("`{text}`");
    }
    if strong {
        text = format!("**{text}**");
    }
    if emphasis {
        text = format!("_{text}_");
    }
    match href.filter(|href| !href.is_empty()) {
        Some(href) => format!("[{text}]({href})"),
        None => text,
    }
}

fn block(node: &Value) -> String {
    let content = children(node);
    match node["type"].as_str() {
        Some("heading") => {
            let level = node["attrs"]["level"].as_u64().unwrap_or(2) as usize;
            format!("{} {}", "#".repeat(level), inline(content))
        }
        Some("bulletList") => content
            .iter()
            .map(|item| format!("- {}", list_item(item)))
            .collect::<Vec<_>>()
            .join("\n"),
        Some("orderedList") => content
            .iter()
            .enumerate()
            .map(|(index, item)| format!("{}. {}", index + 1, list_item(item)))
            .collect::<Vec<_>>()
            .join("\n"),
        Some("taskList") => content
            .iter()
            .map(|task| {
                let done = task["attrs"]["state"].as_str() == Some("DONE");
                format!(
                    "- [{}] {}",
                    if done { "x" } else { " " },
                    inline(children(task))
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some("mediaSingle" | "mediaGroup") => content
            .iter()
            .filter(|media| media["attrs"]["type"].as_str() == Some("external"))
            .filter_map(|media| media["attrs"]["url"].as_str().filter(|url| !url.is_empty()))
            .map(|url| format!("![]({url})"))
            .collect::<Vec<_>>()
            .join("\n\n"),
        Some("codeBlock") => format!("```\n{}\n```", inline(content)),
        Some("rule") => "---".to_string(),
        Some("blockquote") => content
            .iter()
            .map(|child| format!("> {}", block(child)))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => inline(content),
    }
}

fn list_item(item: &Value) -> String {
    let mut text = Vec::new();
    let mut nested = Vec::new();
    for child in children(item) {
        match child["type"].as_str() {
            Some("bulletList" | "orderedList" | "taskList") => nested.push(
                block(child)
                    .lines()
                    .map(|line| format!("   {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => text.push(block(child)),
        }
    }
    let mut out = text.join(" ");
    if !nested.is_empty() {
        out.push('\n');
        out.push_str(&nested.join("\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markdown(json: &str) -> String {
        adf_markdown(&serde_json::from_str(json).expect("json"))
    }

    #[test]
    fn paragraphs_and_marks() {
        assert_eq!(
            markdown(
                r#"{"type":"doc","content":[
                {"type":"paragraph","content":[{"type":"text","text":"Hello "},{"type":"text","text":"world","marks":[{"type":"strong"}]}]},
                {"type":"paragraph","content":[{"type":"text","text":"docs","marks":[{"type":"link","attrs":{"href":"https://example.com"}}]},
                  {"type":"hardBreak"},{"type":"mention","attrs":{"text":"@Sam"}},{"type":"text","text":" x","marks":[{"type":"code"}]}]}
            ]}"#
            ),
            "Hello **world**\n\n[docs](https://example.com)\n**@Sam**` x`"
        );
        assert_eq!(adf_markdown(&serde_json::Value::Null), "");
    }

    #[test]
    fn lists_headings_and_blocks() {
        let text = markdown(
            r#"{"type":"doc","content":[
            {"type":"heading","attrs":{"level":3},"content":[{"type":"text","text":"Steps"}]},
            {"type":"orderedList","content":[
              {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"one"}]},
                {"type":"bulletList","content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"inner"}]}]}]}]},
              {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"two"}]}]}]},
            {"type":"taskList","content":[{"type":"taskItem","attrs":{"state":"DONE"},"content":[{"type":"text","text":"done"}]},
              {"type":"taskItem","attrs":{"state":"TODO"},"content":[{"type":"text","text":"todo"}]}]},
            {"type":"codeBlock","content":[{"type":"text","text":"make"}]},
            {"type":"rule"},
            {"type":"blockquote","content":[{"type":"paragraph","content":[{"type":"text","text":"quoted"}]}]},
            {"type":"mediaSingle","content":[{"type":"media","attrs":{"type":"external","url":"https://example.com/a.png"}}]}
        ]}"#,
        );
        assert_eq!(
            text,
            "### Steps\n\n1. one\n   - inner\n2. two\n\n- [x] done\n- [ ] todo\n\n```\nmake\n```\n\n---\n\n> quoted\n\n![](https://example.com/a.png)"
        );
    }
}
