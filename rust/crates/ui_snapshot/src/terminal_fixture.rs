//! Terminals that print what a terminal board shows and then wait, each under the process name its tab is
//! titled by: the shell setting points at a dispatcher that starts the next terminal of the board in turn.

use std::path::{Path, PathBuf};

const PATH: &str = "\x1b[34m";
const BRANCH: &str = "\x1b[90m";
const PROMPT: &str = "\x1b[35m";
const OK: &str = "\x1b[32m";
const WARN: &str = "\x1b[33m";
const ERR: &str = "\x1b[31m";
const DIM: &str = "\x1b[90m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

fn prompt(dir: &str, command: &str) -> String {
    format!("{PATH}{dir}{BRANCH} feat-login{PROMPT} > {RESET}{command}")
}

fn paint(color: &str, text: &str) -> String {
    format!("{color}{text}{RESET}")
}

pub fn server_log() -> Vec<String> {
    vec![
        prompt("api", "bin/rails server"),
        paint(DIM, "=> Booting Puma"),
        paint(DIM, "=> Rails 7.2.1 application starting in development"),
        paint(DIM, "=> Run `bin/rails server --help` for more startup options"),
        "Puma starting in single mode...".into(),
        "* Listening on http://127.0.0.1:41207".into(),
        paint(OK, "Use Ctrl-C to stop"),
        paint(BOLD, "Started GET \"/users\" for 127.0.0.1"),
        "Processing by UsersController#index as JSON".into(),
        paint(DIM, "  User Load (0.6ms)  SELECT \"users\".* FROM \"users\" WHERE \"users\".\"active\" = TRUE ORDER BY \"users\".\"name\" ASC"),
        format!("{} in 14ms (Views: 0.2ms | ActiveRecord: 0.6ms)", paint(OK, "Completed 200 OK")),
        paint(BOLD, "Started PATCH \"/users/7\" for 127.0.0.1"),
        paint(WARN, "Unpermitted parameter: :role"),
        format!("{} in 22ms", paint(OK, "Completed 200 OK")),
    ]
}

pub fn shell() -> Vec<String> {
    vec![
        prompt("myproject", "git status --short"),
        format!(
            "{} api/app/controllers/users_controller.rb",
            paint(WARN, " M")
        ),
        format!("{} api/spec/requests/users_spec.rb", paint(OK, "A ")),
        prompt("myproject", "pom ports"),
        paint(DIM, "SERVICE        PORT    HOST"),
        "api/server     41207   api.feat-login.myproject.localhost".into(),
        "web/app        41208   web.feat-login.myproject.localhost".into(),
        "postgres       5432    shared".into(),
        prompt("myproject", ""),
    ]
}

pub fn rspec() -> Vec<String> {
    vec![
        prompt("api", "bundle exec rspec spec/requests/users_spec.rb"),
        String::new(),
        format!("{}{}", paint(OK, ".."), paint(ERR, "F")),
        String::new(),
        "Failures:".into(),
        String::new(),
        "  1) Users PATCH /users/:id updates the email".into(),
        paint(
            ERR,
            "     Failure/Error: expect(user.reload.email).to eq(\"ada@example.com\")",
        ),
        String::new(),
        paint(ERR, "       expected: \"ada@example.com\""),
        paint(ERR, "            got: \"ada@old.example.com\""),
        String::new(),
        paint(
            DIM,
            "     # ./spec/requests/users_spec.rb:18:in `block (3 levels) in <top (required)>'",
        ),
        String::new(),
        "Finished in 0.41 seconds (files took 1.62 seconds to load)".into(),
        paint(ERR, "3 examples, 1 failure"),
        prompt("api", ""),
    ]
}

pub fn migrate() -> Vec<String> {
    vec![
        prompt("api", "bin/rails db:migrate"),
        paint(DIM, "== 20261020120000 AddRoleToUsers: migrating ================================"),
        paint(DIM, "-- add_column(:users, :role, :string, {null: false})"),
        paint(ERR, "bin/rails aborted!"),
        paint(ERR, "StandardError: An error has occurred, this and all later migrations canceled:"),
        String::new(),
        paint(ERR, "PG::NotNullViolation: ERROR:  column \"role\" of relation \"users\" contains null values"),
        paint(DIM, "/api/db/migrate/20261020120000_add_role_to_users.rb:3:in `change'"),
        String::new(),
        paint(DIM, "Tasks: TOP => db:migrate"),
        paint(DIM, "(See full trace by running task with --trace)"),
    ]
}

pub fn dev_server() -> Vec<String> {
    vec![
        prompt("web", "pnpm dev"),
        String::new(),
        paint(OK, "  VITE v5.4.8  ready in 412 ms"),
        String::new(),
        "  > Local:   http://127.0.0.1:41208/".into(),
    ]
}

/// One terminal of a board: its directory under the project, the program its tab names and the argument
/// after it, what it printed, and the code it exits with, if it does.
pub struct Term {
    pub dir: &'static str,
    pub program: &'static str,
    pub argument: Option<&'static str>,
    pub lines: Vec<String>,
    pub exit: Option<u8>,
}

/// Writes the programs and the dispatcher for `terms` and returns the shell setting that starts them in turn.
pub fn create(scratch: &Path, terms: &[Term]) -> anyhow::Result<String> {
    let player = std::env::current_exe()?
        .parent()
        .map(|dir| dir.join("term_player"))
        .ok_or_else(|| anyhow::anyhow!("no term_player next to ui_snapshot"))?;
    let bin = scratch.join("termbin");
    std::fs::create_dir_all(&bin)?;
    let mut cases = String::new();
    for (index, term) in terms.iter().enumerate() {
        let program = bin.join(term.program);
        if !program.exists() {
            std::fs::copy(&player, &program)?;
        }
        let output = scratch.join(format!("term-{index}.txt"));
        // The last line is the prompt the cursor waits on, so it gets no line break.
        std::fs::write(&output, term.lines.join("\r\n"))?;
        let exit = term
            .exit
            .map(|code| format!("TERMPLAY_EXIT={code} "))
            .unwrap_or_default();
        let argument = term
            .argument
            .map(|argument| format!(" '{argument}'"))
            .unwrap_or_default();
        cases += &format!(
            "{index}) TERMPLAY_FILE='{}' {exit}exec '{}'{argument} ;;\n",
            output.display(),
            program.display()
        );
    }
    let counter = scratch.join("term-count");
    let dispatcher: PathBuf = scratch.join("termsh");
    std::fs::write(
        &dispatcher,
        format!(
            "n=$(cat '{counter}' 2>/dev/null || echo 0)\necho $((n + 1)) > '{counter}'\ncase $n in\n{cases}esac\n",
            counter = counter.display()
        ),
    )?;
    Ok(format!("/bin/sh {}", dispatcher.display()))
}
