use std::path::Path;

use crate::resolution_root::ResolutionRoot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskEntryParts {
    pub(crate) id: String,
    pub(crate) priority: String,
    pub(crate) status: String,
    pub(crate) slug: String,
    pub(crate) path: String,
    pub(crate) source_ref: Option<String>,
    pub(crate) content: Option<String>,
}

pub(crate) fn discover_task_dir(root: &ResolutionRoot, fallback_cwd: &Path) -> String {
    let mut candidates: Vec<String> = root
        .all_paths()
        .into_iter()
        .filter_map(|path| {
            let mut parts = path.split('/');
            let dir = parts.next()?;
            let file = parts.next()?;
            if parts.next().is_none() && file == taskmd_core::constants::TEMPLATE_FILENAME {
                Some(dir.to_string())
            } else {
                None
            }
        })
        .collect();
    candidates.sort();
    if candidates
        .iter()
        .any(|name| name == taskmd_core::constants::DEFAULT_TASKS_DIR_NAME)
    {
        taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string()
    } else {
        candidates.into_iter().next().unwrap_or_else(|| {
            taskmd_core::discover::discover_or_default(fallback_cwd)
                .to_string_lossy()
                .into_owned()
        })
    }
}

pub(crate) fn list_task_entries(
    root: &ResolutionRoot,
    fallback_cwd: &Path,
    tasks_dir_name: &str,
    limit: Option<usize>,
) -> Vec<TaskEntryParts> {
    let tasks_prefix = format!("{}/", tasks_dir_name.trim_end_matches('/'));
    root.all_paths()
        .into_iter()
        .filter(|repo_relative_path| repo_relative_path.starts_with(&tasks_prefix))
        .filter_map(|repo_relative_path| {
            let filename = Path::new(&repo_relative_path)
                .file_name()
                .and_then(|name| name.to_str())?;
            let parsed = taskmd_core::filename::parse_filename(filename)?;
            let content = root
                .read_text(&repo_relative_path)
                .map(|text| text.trim_end_matches(['\r', '\n']).to_string());
            Some(TaskEntryParts {
                id: parsed.id,
                priority: parsed.priority.to_string(),
                status: parsed.status.to_string(),
                slug: parsed.slug,
                path: fallback_cwd
                    .join(&repo_relative_path)
                    .to_string_lossy()
                    .into_owned(),
                source_ref: root.source_ref().map(str::to_string),
                content,
            })
        })
        .take(limit.unwrap_or(usize::MAX))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn discovers_custom_task_dir_from_read_root() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir_all(repo.path().join("workitems")).unwrap();
        std::fs::write(repo.path().join("workitems/_TEMPLATE.md"), "template").unwrap();
        let root = ResolutionRoot::working_dir(repo.path());
        assert_eq!(discover_task_dir(&root, repo.path()), "workitems");
    }

    #[test]
    fn lists_content_from_same_read_root() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir_all(repo.path().join("tasks")).unwrap();
        std::fs::write(repo.path().join("tasks/00001-p1-ready--demo.md"), "hello").unwrap();
        let root = ResolutionRoot::working_dir(repo.path());
        let entries = list_task_entries(&root, repo.path(), "tasks", None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "00001");
        assert_eq!(entries[0].content.as_deref(), Some("hello"));
        assert_eq!(entries[0].source_ref, None);
    }
}
