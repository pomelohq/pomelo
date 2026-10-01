use crate::site_safe;

const CONFIG_INTRO: &str = "\
# pom.yml reference

Every key Pomelo reads from `pom.yml`, the file at the project root that says how to build and run the
project. It holds only that: the app's own behavior lives in Settings. Values can use
[templates](./templates.md). Pomelo ignores a key not listed here.
";

const TEMPLATES_INTRO: &str = "# Templates\n\n";

pub fn render_config() -> String {
    format!(
        "{CONFIG_INTRO}\n{}",
        site_safe(&pom_config::field_docs::markdown())
    )
}

pub fn render_templates() -> String {
    format!(
        "{TEMPLATES_INTRO}{}",
        site_safe(&pom_env::template_docs::markdown())
    )
}
