use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub const DEBOUNCE: Duration = Duration::from_millis(350);

pub struct Handle {
    _watcher: RecommendedWatcher,
    rx: Receiver<Result<Event, notify::Error>>,
    path: PathBuf,
}

impl Handle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn drain(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            if let Ok(event) = result {
                if matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) {
                    paths.extend(event.paths);
                }
            }
        }
        paths
    }
}

pub fn watch(path: PathBuf) -> Result<Handle, String> {
    let watch_target = if path.is_file() {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.clone())
    } else {
        path.clone()
    };
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        Config::default().with_poll_interval(Duration::from_millis(500)),
    )
    .map_err(|err| format!("file watch: {err}"))?;
    watcher
        .watch(&watch_target, RecursiveMode::NonRecursive)
        .map_err(|err| format!("file watch {}: {err}", watch_target.display()))?;
    Ok(Handle {
        _watcher: watcher,
        rx,
        path,
    })
}

pub fn targets_file(changed: &Path, loaded: &Path) -> bool {
    changed.file_name().is_some_and(|name| {
        loaded
            .file_name()
            .is_some_and(|loaded_name| name == loaded_name)
    }) || paths_same(changed, loaded)
}

fn paths_same(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
