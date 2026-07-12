use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

const SESSION_DIRS: [&str; 2] = ["local-agent-mode-sessions", "claude-code-sessions"];
const SKIPPED_DIRS: [&str; 7] = [
    ".build",
    ".git",
    "build",
    "DerivedData",
    "node_modules",
    "outputs",
    "target",
];

pub fn project_patterns(home: &Path) -> Vec<String> {
    let mut patterns = vec![
        home.join(".claude/projects/**/*.jsonl"),
        home.join(".config/claude/projects/**/*.jsonl"),
    ];
    for root in embedded_project_roots(home) {
        patterns.push(root.join("**/*.jsonl"));
    }
    patterns
        .into_iter()
        .map(|path| path.display().to_string())
        .collect()
}

pub fn embedded_project_roots(home: &Path) -> Vec<PathBuf> {
    let base = home.join("Library/Application Support/Claude");
    let roots: Vec<PathBuf> = SESSION_DIRS.iter().map(|name| base.join(name)).collect();
    let mut queue: VecDeque<(PathBuf, usize)> =
        roots.iter().cloned().map(|path| (path, 0)).collect();
    let mut visited: HashSet<PathBuf> = roots.into_iter().collect();
    let mut projects = Vec::new();

    while let Some((current, depth)) = queue.pop_front() {
        let candidate = current.join(".claude/projects");
        if candidate.is_dir() {
            projects.push(candidate);
        }
        if depth >= 4 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if SKIPPED_DIRS.contains(&name) || !path.is_dir() {
                continue;
            }
            if visited.insert(path.clone()) {
                queue.push_back((path, depth + 1));
            }
        }
    }
    projects
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::embedded_project_roots;

    #[test]
    fn finds_embedded_projects_without_following_skipped_dirs() {
        let root = std::env::temp_dir().join(format!("agent-usage-desktop-{}", std::process::id()));
        let projects = root.join(
            "Library/Application Support/Claude/local-agent-mode-sessions/work/.claude/projects",
        );
        let skipped = root
            .join("Library/Application Support/Claude/local-agent-mode-sessions/work/target/.claude/projects");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&skipped).unwrap();

        let roots = embedded_project_roots(&root);
        assert_eq!(roots, vec![projects]);
        let _ = fs::remove_dir_all(root);
    }
}
