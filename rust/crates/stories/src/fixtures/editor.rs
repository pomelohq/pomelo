//! The files the editor boards show: api and web under one project folder, api a repository whose working copy
//! differs from its last commit where the boards draw diff marks.

use std::path::Path;
use std::process::Command;

pub const CONTROLLER: &str = r#"class UsersController < ApplicationController
  before_action :authenticate_user!
  before_action :set_user, only: %i[show update]

  def index
    @users = User.where(active: true).order(:name)
    render json: @users
  end

  def show
    render json: @user
  end

  def update
    if @user.update(user_params)
      render json: @user
    else
      render json: @user.errors, status: :unprocessable_entity
    end
  end

  private

  def set_user
    @user = User.find(params[:id])
  end

  def user_params
    params.require(:user).permit(:name, :email)
  end
end"#;

pub const SPEC: &str = r#"require "rails_helper"

describe "Users", type: :request do
  let(:user) { create(:user, name: "Ada") }

  describe "GET /users" do
    it "lists active users by name" do
      create(:user, name: "Grace")
      get "/users"
      expect(response).to have_http_status(:ok)
      expect(json.map { _1["name"] }).to eq(%w[Ada Grace])
    end
  end

  describe "PATCH /users/:id" do
    it "updates the email" do
      patch "/users/#{user.id}", params: { user: { email: "ada@example.com" } }
      expect(user.reload.email).to eq("ada@example.com")
    end
  end
end"#;

pub const API_TS: &str = r#"import { request } from "./client";
import type { User } from "../types";

export type Page = { page: number; per: number };

// The server caps per at 100; ask for less and page instead.
export async function fetchUsers({ page, per }: Page): Promise<User[]> {
  const res = await request(`/users?page=${page}&per=${per}`);
  if (!res.ok) throw new Error(`users: ${res.status}`);
  return res.json();
}

export async function activeUsers(): Promise<User[]> {
  const users = await fetchUsers({ page: 1, per: pageSize });
  return users.fi
}"#;

pub const README: &str = r#"# api

The Rails service behind web. Runs on the port Pomelo leases for it.

## Run it

- `bin/setup` once per workspace
- `bin/rails server` or start it from the Services panel

## Endpoints

| Method | Path | Notes |
| --- | --- | --- |
| GET | /users | active users, by name |
| PATCH | /users/:id | name, email |

```sh
curl http://api.feat-login.myproject.localhost/users
```"#;

fn git(root: &Path, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(["-c", "user.name=alex", "-c", "user.email=alex@example.com"])
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_DATE", "2026-10-10T10:00:00")
        .env("GIT_COMMITTER_DATE", "2026-10-10T10:00:00")
        .status()?;
    anyhow::ensure!(status.success(), "git {} failed", args.join(" "));
    Ok(())
}

fn write(root: &Path, path: &str, contents: impl AsRef<[u8]>) -> anyhow::Result<()> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(())
}

/// The controller as last committed: the boards mark lines 6 and 7 as changed since.
fn committed_controller() -> String {
    CONTROLLER.replacen(
        "    @users = User.where(active: true).order(:name)\n    render json: @users\n",
        "    @users = User.all\n    render json: @users, status: :ok\n",
        1,
    )
}

/// The spec as last committed: the split board marks lines 7 to 11 as added since.
fn committed_spec() -> String {
    let lines: Vec<&str> = SPEC.lines().collect();
    [&lines[..6], &lines[11..]].concat().join("\n")
}

pub fn create(root: &Path) -> anyhow::Result<()> {
    if root.exists() {
        std::fs::remove_dir_all(root)?;
    }
    let api = root.join("api");
    write(
        &api,
        "app/controllers/users_controller.rb",
        committed_controller(),
    )?;
    write(
        &api,
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::API\nend\n",
    )?;
    write(
        &api,
        "app/models/user.rb",
        "class User < ApplicationRecord\nend\n",
    )?;
    write(
        &api,
        "config/routes.rb",
        "Rails.application.routes.draw do\n  resources :users, only: %i[index show update]\nend\n",
    )?;
    write(&api, "spec/requests/users_spec.rb", committed_spec())?;
    write(
        &api,
        "spec/models/user_spec.rb",
        "require \"rails_helper\"\n",
    )?;
    write(&api, "Gemfile", "source \"https://rubygems.org\"\n")?;
    write(&api, "README.md", README)?;
    git(&api, &["init", "-q", "-b", "main"])?;
    git(&api, &["add", "-A"])?;
    git(&api, &["commit", "-q", "-m", "Initial api"])?;
    write(&api, "app/controllers/users_controller.rb", CONTROLLER)?;
    write(&api, "spec/requests/users_spec.rb", SPEC)?;
    write(
        &api,
        ".env.local",
        "DATABASE_URL=postgres://localhost/api\n",
    )?;
    let web = root.join("web");
    write(&web, "src/api/users.ts", API_TS)?;
    write(&web, "src/api/users", API_TS)?;
    write(&web, "package.json", "{ \"name\": \"web\" }\n")?;
    write(
        &web,
        "public/logo.png",
        include_bytes!("../../../workspace/assets/logo.png"),
    )?;
    write(&web, "public/broken.png", b"not a png")?;
    write(root, "pom.yml", "name: myproject\n")?;
    Ok(())
}

/// The three files the project search boards find `User.where(active: true)` in.
pub fn create_search(root: &Path) -> anyhow::Result<()> {
    let files = [
        (
            "api/app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  before_action :set_user, only: :show\n\n  def index\n    @users = User.where(active: true).order(:name)\n    render json: @users\n  end\n\n  def show\n    render json: @user\n  end\n\n  private\n\n  def set_user\n    @user = User.where(active: true).find(params[:id])\n  end\nend\n",
        ),
        (
            "api/spec/requests/users_spec.rb",
            "require \"rails_helper\"\n\nRSpec.describe \"Users\", type: :request do\n  describe \"GET /users\" do\n    before { create_list(:user, 30) }\n\n    it \"lists active users\" do\n      get \"/users\"\n      expect(User.where(active: true).count).to eq(30)\n    end\n  end\nend\n",
        ),
        (
            "web/src/api/users.ts",
            "import { client } from \"./client\";\n\nexport async function activeUsers() {\n  // mirrors User.where(active: true) on the server\n  return client.get(\"/users\");\n}\n",
        ),
    ];
    for (path, text) in files {
        write(root, path, text)?;
    }
    Ok(())
}

/// The project diagnostics tab over two files: two errors in the TypeScript client, a warning in the controller.
pub fn project_diagnostics() -> Box<dyn workspace::Item> {
    files_ui::project_diagnostics_preview(vec![
        (
            "web/src/api/users.ts".into(),
            API_TS.to_string(),
            vec![
                (12, 41, 56, true, "A function whose declared type is neither 'undefined', 'void', nor 'any' must return a value.", "ts", "2355"),
                (13, 44, 52, true, "Cannot find name 'pageSize'.", "ts", "2304"),
            ],
        ),
        (
            "api/app/controllers/users_controller.rb".into(),
            CONTROLLER.to_string(),
            vec![(
                14,
                4,
                6,
                false,
                "Style/GuardClause: Use a guard clause instead of wrapping the code inside a conditional expression.",
                "rubocop",
                "",
            )],
        ),
    ])
}

/// The hover over the undefined `pageSize`: its error, then the function's signature and doc.
pub fn hover() -> Vec<(ui::Node, ui::Rect)> {
    files_ui::hover_preview(
        "web/src/api/users.ts",
        API_TS,
        (13, 44),
        vec![(13, 44, 52, true, "Cannot find name 'pageSize'.", "ts", "2304")],
        "```typescript\nfunction fetchUsers({ page, per }: Page): Promise<User[]>\n```\n\nThe server caps per at 100; ask for less and page instead.",
        (1092.0, 770.0),
    )
}

/// The completions for `query` after `users.`, with the chosen one's doc beside them.
pub fn completion(query: &str) -> Vec<(ui::Node, ui::Rect)> {
    files_ui::completion_preview(
        vec![
            ("filter", "(predicate) => User[]", false),
            ("find", "(predicate) => User", false),
            ("findIndex", "(predicate) => number", false),
            ("findLast", "(predicate) => User", false),
            ("fill", "(value, start?, end?)", false),
            ("flat", "(depth?) => User[]", false),
            ("for in", "snippet", true),
        ],
        query,
        "```typescript\nfind(predicate): User | undefined\n```\n\nReturns the value of the first element in the array where predicate is true, and undefined otherwise.",
    )
}

/// Popovers where the editor places them, raised with their shadow, inset so the shadow shows.
pub fn paint_popovers(popovers: Vec<(ui::Node, ui::Rect)>) -> ui::Painted {
    let mut painted = ui::Painted::default();
    for (node, rect) in popovers {
        let rect = ui::Rect::new(rect.x + 24.0, rect.y + 24.0, rect.w, rect.h, rect.color);
        painted
            .rects
            .extend(ui::elevation_shadow(rect, ui::Elevation::Elevated, 1.0));
        let part = ui::render(&ui::div().col().child(node).into(), rect);
        painted.rects.extend(part.rects);
        painted.tris.extend(part.tris);
        painted.texts.extend(part.texts);
        painted.icons.extend(part.icons);
    }
    painted
}
