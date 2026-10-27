//! The forms that make and change workspaces: create with its ticket and branch pickers, rename, add a repo,
//! use a branch and pick repos. Jira and git answer from fixtures.

use crate::fixtures::forms;
use crate::page::modal;
use crate::StoryDef;

const CREATE: (f32, f32) = (660.0, 600.0);
const ADD_REPO: (f32, f32) = (580.0, 440.0);
const SMALL: (f32, f32) = (500.0, 300.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, size, build| StoryDef {
        id,
        page: "Organisms",
        group: "Workspace forms",
        title,
        size,
        build,
    };
    vec![
        story("workspace-form-create", "Create", CREATE, || {
            modal(forms::modal("create"))
        }),
        story(
            "workspace-form-create-tickets",
            "Create: ticket picker",
            CREATE,
            || modal(forms::modal("create-tickets")),
        ),
        story(
            "workspace-form-create-tickets-loading",
            "Create: tickets loading",
            CREATE,
            || modal(forms::modal("create-tickets-loading")),
        ),
        story(
            "workspace-form-create-tickets-error",
            "Create: Jira failed",
            CREATE,
            || modal(forms::modal("create-tickets-error")),
        ),
        story(
            "workspace-form-create-branch",
            "Create: branch picker",
            CREATE,
            || modal(forms::modal("create-branch")),
        ),
        story(
            "workspace-form-create-taken",
            "Create: branch taken",
            CREATE,
            || modal(forms::modal("create-taken")),
        ),
        story("workspace-form-rename", "Rename", SMALL, || {
            modal(forms::modal("rename"))
        }),
        story(
            "workspace-form-add-repo-blank",
            "Add repo",
            ADD_REPO,
            || modal(forms::modal("add-repo-blank")),
        ),
        story(
            "workspace-form-add-repo",
            "Add repo: filled in",
            ADD_REPO,
            || modal(forms::modal("add-repo")),
        ),
        story("workspace-form-use-branch", "Use branch", SMALL, || {
            modal(forms::modal("use-branch"))
        }),
        story("workspace-form-pick-repos", "Pick repos", SMALL, || {
            modal(forms::modal("pick-repos"))
        }),
    ]
}
