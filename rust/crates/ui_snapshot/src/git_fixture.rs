//! Scratch repositories holding the files the Git boards show, put in the state a board draws.

use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT: &str = "/tmp/pomelo-qa-fixture/git";

const CONTROLLER: &str = "class UsersController < ApplicationController
  before_action :authenticate_user!

  def index
    @users = User.where(active: true).order(:name)
    render json: @users
  end

  def show
    @user = User.find(params[:id])
  end

  def create
    @user = User.create!(user_params)
  end

  def update
    if @user.update(user_params)
      render json: @user
    end
  end
end
";

const PAGINATED_INDEX: &str = "    page = params.fetch(:page, 1).to_i
    per = params.fetch(:per, 25).to_i.clamp(1, 100)
    @users = User.active.order(:name)
                 .offset((page - 1) * per).limit(per)
";

const AUTHORIZED_UPDATE: &str = "  def update
    authorize! @user
    @user.touch
";

fn paginated(controller: &str) -> String {
    controller.replacen(
        "    @users = User.where(active: true).order(:name)\n",
        PAGINATED_INDEX,
        1,
    )
}

fn authorized(controller: &str) -> String {
    controller.replacen("  def update\n", AUTHORIZED_UPDATE, 1)
}

fn numbered(prefix: &str, count: usize) -> String {
    (1..=count)
        .map(|line| format!("{prefix} {line}\n"))
        .collect()
}

fn schema(edited: bool) -> String {
    let mut text = String::from("ActiveRecord::Schema[7.1].define(version: 2026_10_01) do\n");
    for (column, line) in ["name", "email", "role"].iter().enumerate() {
        text += &if edited {
            format!("  t.string \"{line}\", null: false # {column}\n")
        } else {
            format!("  t.string \"{line}\" # {column}\n")
        };
    }
    text += &numbered("  # column", 6);
    text + "end\n"
}

fn user_list(edited: bool) -> String {
    let mut text = numbered("// UserList line", 12);
    if edited {
        text += &numbered("// paged row", 4);
        text += &numbered("// pager control", 14);
    } else {
        text += &numbered("// plain row", 4);
    }
    text + &numbered("// UserList tail", 14)
}

fn logo(edited: bool) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0, 0, 0, 0];
    bytes.push(if edited { 2 } else { 1 });
    bytes
}

/// A board's state over the same files: what is staged, what is committed, which commit is the newest.
#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Unstaged,
    Staged,
    StagedAll,
    Committed,
}

impl State {
    pub fn parse(name: &str) -> Option<State> {
        Some(match name {
            "unstaged" => State::Unstaged,
            "staged" => State::Staged,
            "staged-all" => State::StagedAll,
            "committed" => State::Committed,
            _ => return None,
        })
    }
}

struct Repo {
    root: PathBuf,
    clock: u32,
    /// Seconds before now for the next commit; when set it wins over `clock`, so relative times read as drawn.
    ago: Option<u64>,
    author: &'static str,
}

impl Repo {
    fn create(name: &str) -> anyhow::Result<Repo> {
        let root = Path::new(ROOT).join(name);
        std::fs::create_dir_all(&root)?;
        let repo = Repo {
            root,
            clock: 0,
            ago: None,
            author: "alex",
        };
        repo.git(&["init", "-q", "-b", "main"])?;
        Ok(repo)
    }

    fn git(&self, args: &[&str]) -> anyhow::Result<()> {
        let date = match self.ago {
            Some(ago) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or_default();
                format!("@{} +0000", now.saturating_sub(ago))
            }
            None => format!("2026-10-{:02}T10:00:00", 10 + self.clock),
        };
        let email = format!("{}@example.com", self.author);
        let status = Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(["-c", &format!("user.name={}", self.author)])
            .args(["-c", &format!("user.email={email}")])
            .args(args)
            .current_dir(&self.root)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .status()?;
        anyhow::ensure!(status.success(), "git {} failed", args.join(" "));
        Ok(())
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) -> anyhow::Result<()> {
        let path = self.root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        Ok(())
    }

    fn commit_as(&mut self, subject: &str, author: &'static str, ago: u64) -> anyhow::Result<()> {
        self.author = author;
        self.ago = Some(ago);
        self.git(&["add", "-A"])?;
        let done = self.git(&["commit", "-q", "-m", subject]);
        self.author = "alex";
        done
    }

    fn commit(&mut self, subject: &str, day: u32) -> anyhow::Result<()> {
        self.clock = day;
        self.git(&["commit", "-q", "-m", subject])
    }
}

/// Writes the api and web repositories on feat-login in `state` and returns their roots.
pub fn create(state: State) -> anyhow::Result<Vec<PathBuf>> {
    let root = Path::new(ROOT);
    if root.exists() {
        std::fs::remove_dir_all(root)?;
    }

    let mut api = Repo::create("api")?;
    api.write("app/controllers/users_controller.rb", CONTROLLER)?;
    api.write("db/schema.rb", schema(false))?;
    api.git(&["add", "-A"])?;
    api.commit("Initial api", 1)?;
    api.git(&["checkout", "-q", "-b", "feat-login"])?;
    api.write("db/seeds.rb", numbered("User.create!(name: \"user\")", 30))?;
    api.git(&["add", "-A"])?;
    api.commit("Seed 30 users for request specs", 5)?;

    let mut web = Repo::create("web")?;
    web.write("src/users/UserList.tsx", user_list(false))?;
    web.write("src/users/legacyTable.css", numbered(".legacy-row", 57))?;
    web.write("public/logo.png", logo(false))?;
    web.git(&["add", "-A"])?;
    web.commit("Initial web", 1)?;
    web.git(&["checkout", "-q", "-b", "feat-login"])?;
    web.write("src/users/index.ts", "export * from \"./UserList\";\n")?;
    web.git(&["add", "-A"])?;
    web.commit("Users list pagination controls", 3)?;

    let edited_controller = authorized(&paginated(CONTROLLER));
    if state == State::Staged {
        api.write("app/controllers/users_controller.rb", paginated(CONTROLLER))?;
        api.git(&["add", "app/controllers/users_controller.rb"])?;
    }
    api.write("app/controllers/users_controller.rb", &edited_controller)?;
    api.write("spec/requests/users_spec.rb", numbered("# users spec", 42))?;
    api.write("db/schema.rb", schema(true))?;
    api.git(&["add", "-N", "spec/requests/users_spec.rb"])?;

    web.write("src/users/UserList.tsx", user_list(true))?;
    web.write(
        "src/users/usePagination.ts",
        numbered("// usePagination", 31),
    )?;
    std::fs::remove_file(web.root.join("src/users/legacyTable.css"))?;
    web.write("public/logo.png", logo(true))?;
    web.git(&["add", "-N", "src/users/usePagination.ts"])?;

    match state {
        State::Unstaged => {}
        State::Staged => {
            api.git(&["add", "spec/requests/users_spec.rb"])?;
            web.git(&["add", "src/users/UserList.tsx"])?;
        }
        State::StagedAll => {
            api.git(&["add", "-A"])?;
            web.git(&["add", "-A"])?;
        }
        State::Committed => {
            api.git(&[
                "add",
                "app/controllers/users_controller.rb",
                "spec/requests/users_spec.rb",
            ])?;
            api.commit("Paginate the users index", 7)?;
            web.git(&["add", "src/users/UserList.tsx"])?;
            web.commit("Paginate the users index", 8)?;
        }
    }
    Ok(vec![api.root, web.root])
}

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// A board on the Remote or History tab: every repo against origin as drawn, or all of them in sync.
#[derive(Clone, Copy, PartialEq)]
pub enum RemoteState {
    Differ,
    InSync,
}

impl RemoteState {
    pub fn parse(name: &str) -> Option<RemoteState> {
        Some(match name {
            "differ" => RemoteState::Differ,
            "in-sync" => RemoteState::InSync,
            _ => return None,
        })
    }
}

/// A bare origin next to the scratch repos, with `repo`'s branches pushed to it.
fn origin(repo: &Repo, name: &str) -> anyhow::Result<PathBuf> {
    let bare = Path::new(ROOT).join("origin").join(format!("{name}.git"));
    std::fs::create_dir_all(&bare)?;
    let status = Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .current_dir(&bare)
        .status()?;
    anyhow::ensure!(status.success(), "git init --bare failed");
    repo.git(&["remote", "add", "origin", &bare.display().to_string()])?;
    repo.git(&["push", "-q", "origin", "main"])?;
    Ok(bare)
}

/// api, web and worker on feat-login against their origins: api two commits ahead, web never published,
/// worker two behind; or, `InSync`, every one matching origin. Each has edits left uncommitted.
pub fn create_remote(state: RemoteState) -> anyhow::Result<Vec<PathBuf>> {
    let root = Path::new(ROOT);
    if root.exists() {
        std::fs::remove_dir_all(root)?;
    }

    let mut api = Repo::create("api")?;
    api.write("app/controllers/users_controller.rb", CONTROLLER)?;
    api.write("db/schema.rb", schema(false))?;
    api.git(&["add", "-A"])?;
    api.commit("Initial api", 1)?;
    origin(&api, "api")?;
    api.git(&["checkout", "-q", "-b", "feat-login"])?;
    api.write("db/schema.rb", schema(true))?;
    api.commit_as("Seed 30 users for request specs", "bea", 3 * DAY)?;
    api.write("app/controllers/users_controller.rb", paginated(CONTROLLER))?;
    api.commit_as(
        "Add per-page clamp to UsersController",
        "sam",
        DAY + 2 * HOUR,
    )?;
    api.git(&["push", "-q", "-u", "origin", "feat-login"])?;
    api.write("db/schema.rb", schema(true) + "  # per <= 100\n")?;
    api.commit_as("Clamp per to 100", "alex", 20 * MINUTE)?;
    api.write(
        "app/controllers/users_controller.rb",
        authorized(&paginated(CONTROLLER)),
    )?;
    api.write("spec/requests/users_spec.rb", numbered("# users spec", 42))?;
    api.commit_as("Paginate the users index", "alex", 4 * MINUTE)?;

    let mut web = Repo::create("web")?;
    web.write("src/users/UserList.tsx", user_list(false))?;
    web.write("src/users/legacyTable.css", numbered(".legacy-row", 57))?;
    web.git(&["add", "-A"])?;
    web.commit("Initial web", 1)?;
    origin(&web, "web")?;
    web.git(&["checkout", "-q", "-b", "feat-login"])?;
    web.write(
        "src/users/usePagination.ts",
        numbered("// usePagination", 31),
    )?;
    web.write("src/users/index.ts", "export * from \"./UserList\";\n")?;
    web.commit_as("Extract usePagination hook", "alex", DAY + 3 * HOUR)?;
    web.write("src/users/UserList.tsx", user_list(true))?;
    std::fs::remove_file(web.root.join("src/users/legacyTable.css"))?;
    web.commit_as("Users list pagination controls", "alex", HOUR)?;

    let mut worker = Repo::create("worker")?;
    worker.write("app/jobs/retry_job.rb", numbered("# retry", 12))?;
    worker.git(&["add", "-A"])?;
    worker.commit("Initial worker", 1)?;
    let worker_origin = origin(&worker, "worker")?;
    worker.git(&["checkout", "-q", "-b", "feat-login"])?;
    worker.git(&["push", "-q", "-u", "origin", "feat-login"])?;
    // Someone else pushes two commits to worker's branch; the local checkout only fetches them.
    let mut other = Repo::create("worker-other")?;
    other.git(&[
        "remote",
        "add",
        "origin",
        &worker_origin.display().to_string(),
    ])?;
    other.git(&["fetch", "-q", "origin"])?;
    other.git(&["checkout", "-q", "-b", "feat-login", "origin/feat-login"])?;
    other.write("Gemfile.lock", "sidekiq (7.3.0)\n")?;
    other.commit_as("Bump sidekiq", "bea", DAY + HOUR)?;
    other.write(
        "app/jobs/retry_job.rb",
        numbered("# retry with backoff", 14),
    )?;
    other.commit_as("Retry failed mail with backoff", "sam", 2 * HOUR)?;
    other.git(&["push", "-q", "origin", "feat-login"])?;
    std::fs::remove_dir_all(&other.root)?;
    worker.git(&["fetch", "-q", "origin"])?;

    if state == RemoteState::InSync {
        api.git(&["push", "-q", "origin", "feat-login"])?;
        web.git(&["push", "-q", "-u", "origin", "feat-login"])?;
        worker.git(&["merge", "-q", "--ff-only", "origin/feat-login"])?;
    }

    // Edits not committed yet, so the Changes tab counts what the boards show.
    let edited = |text: &str| format!("{text}# edited\n");
    api.write(
        "app/controllers/users_controller.rb",
        edited(&authorized(&paginated(CONTROLLER))),
    )?;
    api.write(
        "spec/requests/users_spec.rb",
        edited(&numbered("# users spec", 42)),
    )?;
    api.write("db/schema.rb", edited(&(schema(true) + "  # per <= 100\n")))?;
    web.write("src/users/UserList.tsx", edited(&user_list(true)))?;
    web.write(
        "src/users/usePagination.ts",
        edited(&numbered("// usePagination", 31)),
    )?;
    web.write(
        "src/users/index.ts",
        "export * from \"./UserList\";\nexport * from \"./usePagination\";\n",
    )?;
    Ok(vec![api.root, web.root, worker.root])
}

/// The Changes boards' repos with web left on another branch.
pub fn create_drift() -> anyhow::Result<Vec<PathBuf>> {
    let roots = create(State::Unstaged)?;
    let web = Repo {
        root: roots[1].clone(),
        clock: 0,
        ago: None,
        author: "alex",
    };
    web.git(&["checkout", "-q", "-b", "ana/checkout-ui"])?;
    Ok(roots)
}
