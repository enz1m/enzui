//! Real repositories, built with the `git` command (the tests may run it;
//! the reader never does).

use super::*;
use std::path::Path;
use std::process::Command;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("enzui-git-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    Scratch(dir)
}

// Keep fixture commits independent of user identity and global/system Git configuration.
fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@e",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("dir");
    }
    std::fs::write(path, text).expect("write");
}

// Any git-init failure skips this fixture, including failures beyond a missing binary.
fn repo(tag: &str) -> Option<Scratch> {
    let dir = scratch(tag);
    if !git(&dir.0, &["init", "-q"]) {
        println!("skipped: no git");
        return None;
    }
    write(&dir.0.join("src/main.rs"), "fn main() {}\n");
    write(&dir.0.join("README.md"), "hello\n");
    write(&dir.0.join(".gitignore"), "target/\n*.log\n");
    assert!(git(&dir.0, &["add", "."]) && git(&dir.0, &["commit", "-qm", "first"]));
    Some(dir)
}

fn never() -> bool {
    false
}

fn status_of(dir: &Path) -> StatusSummary {
    discover(dir)
        .expect("a repository")
        .status(StatusBudget::default(), &never)
        .expect("status")
}

fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .ok()
}

fn fifo(path: &Path) {
    let _ = std::fs::remove_file(path);
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o644),
        0,
    )
    .expect("fifo");
}

fn socket(path: &Path) -> std::os::unix::net::UnixListener {
    let _ = std::fs::remove_file(path);
    std::os::unix::net::UnixListener::bind(path).expect("socket")
}

#[test]
fn special_files_in_the_repository_are_refused_without_blocking() {
    let Some(dir) = repo("hostile") else {
        return;
    };
    let git_dir = dir.0.join(".git");
    let work = dir.0.clone();

    // The index as a FIFO: the status is unavailable, in bounded time.
    let index = git_dir.join("index");
    let saved = std::fs::read(&index).unwrap();
    fifo(&index);
    let probe = work.clone();
    let result = within(move || {
        discover(&probe)
            .unwrap()
            .status(StatusBudget::default(), &never)
    })
    .expect("a FIFO index must not block");
    assert!(matches!(result, Err(ReadError::Io(_))), "{result:?}");
    let _listener = socket(&index);
    let probe = work.clone();
    let result = within(move || {
        discover(&probe)
            .unwrap()
            .status(StatusBudget::default(), &never)
    })
    .expect("a socket index must not block");
    assert!(result.is_err());
    std::fs::remove_file(&index).unwrap();
    std::fs::write(&index, saved).unwrap();

    // The config as a FIFO leaves the object format unknown: unavailable,
    // never a confident answer.
    let config = git_dir.join("config");
    let saved = std::fs::read(&config).unwrap();
    fifo(&config);
    let probe = work.clone();
    let result = within(move || {
        discover(&probe)
            .unwrap()
            .status(StatusBudget::default(), &never)
    })
    .expect("a FIFO config must not block");
    assert!(result.is_err(), "{result:?}");
    std::fs::remove_file(&config).unwrap();
    std::fs::write(&config, saved).unwrap();

    // packed-refs as a FIFO: the branch still reads, from loose refs.
    fifo(&git_dir.join("packed-refs"));
    let probe = work.clone();
    let head = within(move || discover(&probe).unwrap().head()).expect("no block on packed-refs");
    assert_eq!(head.unwrap().label(), "main");
    std::fs::remove_file(git_dir.join("packed-refs")).unwrap();

    // A FIFO .gitignore is skipped without blocking status.
    fifo(&work.join(".gitignore"));
    let probe = work.clone();
    let summary = within(move || status_of(&probe)).expect("no block on .gitignore");
    assert!(
        summary.untracked <= 1 && summary.deleted == 0,
        "{summary:?}"
    );

    // HEAD as a FIFO: not a repository at all, and found quickly.
    fifo(&git_dir.join("HEAD"));
    let probe = work;
    let found = within(move || discover(&probe)).expect("no block on HEAD");
    assert_eq!(found.and_then(|repository| repository.work_tree), None);
}
