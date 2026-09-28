use std::collections::BTreeMap;

/// One row of a file tree: a folder (single-child folders joined into its label) or a file, by its index
/// into the paths the tree was built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeItem {
    Directory {
        path: String,
        label: String,
        depth: usize,
        /// Every file under it, by index.
        files: Vec<usize>,
    },
    File {
        index: usize,
        depth: usize,
    },
}

#[derive(Default)]
struct Folder {
    folders: BTreeMap<String, Folder>,
    files: Vec<usize>,
}

impl Folder {
    fn all_files(&self) -> Vec<usize> {
        let mut files = self.files.clone();
        for folder in self.folders.values() {
            files.extend(folder.all_files());
        }
        files
    }
}

/// `paths` as a tree, folders before files at each level; `folded(path)` hides a folder's content.
pub(crate) fn build(paths: &[&str], depth: usize, folded: &dyn Fn(&str) -> bool) -> Vec<TreeItem> {
    let mut root = Folder::default();
    for (index, path) in paths.iter().enumerate() {
        let mut node = &mut root;
        let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        parts.pop();
        for part in parts {
            node = node.folders.entry(part.to_string()).or_default();
        }
        node.files.push(index);
    }
    let mut items = Vec::new();
    walk(&root, depth, "", paths, folded, &mut items);
    items
}

fn walk(
    folder: &Folder,
    depth: usize,
    prefix: &str,
    paths: &[&str],
    folded: &dyn Fn(&str) -> bool,
    items: &mut Vec<TreeItem>,
) {
    for (name, child) in &folder.folders {
        let mut label = name.clone();
        let mut path = format!("{prefix}{name}");
        let mut node = child;
        while node.files.is_empty() && node.folders.len() == 1 {
            let Some((only, next)) = node.folders.iter().next() else {
                break;
            };
            label = format!("{label}/{only}");
            path = format!("{path}/{only}");
            node = next;
        }
        items.push(TreeItem::Directory {
            path: path.clone(),
            label,
            depth,
            files: node.all_files(),
        });
        if !folded(&path) {
            walk(node, depth + 1, &format!("{path}/"), paths, folded, items);
        }
    }
    let mut files = folder.files.clone();
    files.sort_by_key(|index| paths.get(*index).copied().unwrap_or_default());
    items.extend(
        files
            .into_iter()
            .map(|index| TreeItem::File { index, depth }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_child_folders_join_and_folded_folders_hide_their_files() {
        let paths = [
            "app/models/user.rb",
            "app/controllers/a.rb",
            "db/migrate/1.rb",
            "README.md",
        ];
        let items = build(&paths, 0, &|_| false);
        assert_eq!(
            items,
            [
                TreeItem::Directory {
                    path: "app".into(),
                    label: "app".into(),
                    depth: 0,
                    files: vec![1, 0]
                },
                TreeItem::Directory {
                    path: "app/controllers".into(),
                    label: "controllers".into(),
                    depth: 1,
                    files: vec![1]
                },
                TreeItem::File { index: 1, depth: 2 },
                TreeItem::Directory {
                    path: "app/models".into(),
                    label: "models".into(),
                    depth: 1,
                    files: vec![0]
                },
                TreeItem::File { index: 0, depth: 2 },
                TreeItem::Directory {
                    path: "db/migrate".into(),
                    label: "db/migrate".into(),
                    depth: 0,
                    files: vec![2]
                },
                TreeItem::File { index: 2, depth: 1 },
                TreeItem::File { index: 3, depth: 0 },
            ]
        );
        let folded = build(&paths, 1, &|path| path == "app");
        assert_eq!(folded.len(), 4, "{folded:?}");
        assert!(matches!(folded[0], TreeItem::Directory { depth: 1, .. }));
    }
}
