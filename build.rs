use std::process::Command;

fn main() {
    // 當 git HEAD 變更時重新執行 build script
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let git_version = get_git_version();
    println!("cargo:rustc-env=GIT_VERSION={}", git_version);
}

fn get_git_version() -> String {
    // git describe --tags --always --dirty
    // --tags: 使用 annotated 和 lightweight tags
    // --always: 沒有 tag 時 fallback 到 commit hash
    // --dirty: 有未提交的變更時加上 -dirty 後綴
    Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}
