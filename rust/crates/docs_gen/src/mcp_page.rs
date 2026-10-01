use serde_json::Value;

use crate::{code, prose};

const INTRO: &str = "\
# MCP tools

The tools Pomelo's MCP server gives a coding agent working in a workspace.
The app registers the server with Claude Code in `~/.claude.json` (see
**Settings > Agent > MCP Server**); any other MCP client can run
`pom mcp [--branch <branch>]` over stdio. The project is the `pom.yml` above
the directory the server starts in; the workspace is `--branch`, else the
`workspace--<branch>` folder it starts in, else main.

A read-only tool changes nothing. A result longer than the limit shown is cut, and
the full text is saved to a file under `~/.local/state/pom/mcp-out/` whose
path ends the result.
";

pub fn render() -> String {
    let mut page = String::from(INTRO);
    for tool in pom_mcp::catalog() {
        page.push_str(&format!(
            "\n## {}\n\n{}\n",
            tool.name,
            prose(tool.description)
        ));
        let mut traits = vec![if tool.read_only {
            "Read-only".to_string()
        } else if tool.destructive {
            "Not read-only; destructive".to_string()
        } else {
            "Not read-only".to_string()
        }];
        if tool.max_result_chars > 0 {
            traits.push(format!(
                "results over {} characters are cut",
                tool.max_result_chars
            ));
        }
        page.push_str(&format!("\n{}.\n", traits.join("; ")));
        let arguments = tool.schema.as_ref().map(arguments).unwrap_or_default();
        if !arguments.is_empty() {
            page.push_str(
                "\n| Argument | Type | Required | Description |\n| --- | --- | --- | --- |\n",
            );
            for argument in arguments {
                page.push_str(&argument);
            }
        }
    }
    page
}

fn arguments(schema: &Value) -> Vec<String> {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, property)| {
            let kind = property
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("any");
            let description = property
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("-");
            let required = if required.contains(&name.as_str()) {
                "yes"
            } else {
                "no"
            };
            format!(
                "| {} | {kind} | {required} | {} |\n",
                code(name),
                prose(description)
            )
        })
        .collect()
}
