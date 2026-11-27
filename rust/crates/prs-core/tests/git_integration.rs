use prs_core::{
    DiffLineKind, GitDiffService, LocalRepositoryService, NoCredentialProvider, ProcessResult,
    ProcessRunner, PullRequestDetails, PullRequestReference, ReviewProvider, ReviewSessionState,
    SymbolNavigationService,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from("../../../target")
            .join("prs-core-integration")
            .join(format!(
                "{}-{}-{label}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn reads_changed_files_diff_file_and_definition_from_git_objects() {
    let root = TestDirectory::new("read-diff-definition");
    let repository = root.path().join("review");
    std::fs::create_dir_all(repository.join("src")).unwrap();
    initialize_repository(&repository).await;
    write_file(
        &repository.join("src/MathCore.cs"),
        "public sealed class MathCore\n{\n    public static int Calculate(int value) => value * 2;\n}\n",
    );
    write_file(
        &repository.join("src/Consumer.cs"),
        "public sealed class Consumer\n{\n    public int Read() => 1;\n}\n",
    );
    git(&repository, &["add", "."]).await;
    git(&repository, &["commit", "--quiet", "-m", "base"]).await;
    let base = git_stdout(&repository, &["rev-parse", "HEAD"]).await;
    write_file(
        &repository.join("src/Consumer.cs"),
        "public sealed class Consumer\n{\n    public int Read(int value) => MathCore.Calculate(value);\n}\n",
    );
    git(&repository, &["add", "src/Consumer.cs"]).await;
    git(&repository, &["commit", "--quiet", "-m", "review"]).await;
    let head = git_stdout(&repository, &["rev-parse", "HEAD"]).await;
    let state = review_state(&repository, &base, &head);

    let service = GitDiffService::new(ProcessRunner::new());
    let changed = service.list_changed_files(&state).await.unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].path, "src/Consumer.cs");

    let diff = service
        .get_diff(&state, &changed[0].path, 3, false)
        .await
        .unwrap();
    assert_eq!(diff.len(), 1);
    assert!(
        diff[0].lines.iter().any(
            |line| line.kind == DiffLineKind::Added && line.text.contains("MathCore.Calculate")
        )
    );
    assert_eq!(
        service
            .read_file(&state, &head, "src\\Consumer.cs")
            .await
            .unwrap(),
        "public sealed class Consumer\n{\n    public int Read(int value) => MathCore.Calculate(value);\n}\n"
    );

    let definition = SymbolNavigationService::new(ProcessRunner::new())
        .find_definition(&state, "Calculate", "src/Consumer.cs")
        .await
        .unwrap()
        .expect("definition should be found");
    assert_eq!(definition.file_path, "src/MathCore.cs");
    assert!(definition.line_text.contains("static int Calculate"));
}

#[tokio::test]
async fn definition_navigation_rejects_call_sites_and_finds_late_declarations() {
    let root = TestDirectory::new("definition-navigation");
    let repository = root.path().join("review");
    std::fs::create_dir_all(repository.join("src")).unwrap();
    initialize_repository(&repository).await;
    write_file(
        &repository.join("src/revealtestapi.cpp"),
        "#include <algorithm>\n\nint Read(int value)\n{\n    return std::clamp(value, 0, 10);\n}\n",
    );
    git(&repository, &["add", "."]).await;
    git(&repository, &["commit", "--quiet", "-m", "call-only"]).await;
    let commit = git_stdout(&repository, &["rev-parse", "HEAD"]).await;
    let state = review_state(&repository, &commit, &commit);
    let service = SymbolNavigationService::new(ProcessRunner::new());
    assert!(
        service
            .find_definition_async(
                &state,
                "clamp",
                "src/revealtestapi.cpp",
                Some(&commit),
                Some(5),
                None,
            )
            .await
            .unwrap()
            .is_none()
    );

    let calls = (1..=300)
        .map(|index| format!("    values[{}] = clamp({index});", index - 1))
        .collect::<Vec<_>>()
        .join("\n");
    let late_declaration = format!(
        "#include <array>\n\nint Run()\n{{\n{calls}\n    return 0;\n}}\n\nint clamp(int value)\n{{\n    return value;\n}}\n"
    );
    write_file(&repository.join("src/revealtestapi.cpp"), &late_declaration);
    git(&repository, &["add", "."]).await;
    git(
        &repository,
        &["commit", "--quiet", "-m", "late declaration"],
    )
    .await;
    let commit = git_stdout(&repository, &["rev-parse", "HEAD"]).await;
    let state = review_state(&repository, &commit, &commit);
    let definition = service
        .find_definition_async(
            &state,
            "clamp",
            "src/revealtestapi.cpp",
            Some(&commit),
            Some(6),
            None,
        )
        .await
        .unwrap()
        .expect("late declaration should be found");
    assert_eq!(definition.file_path, "src/revealtestapi.cpp");
    assert!(definition.line_text.contains("int clamp"));
}

#[tokio::test]
async fn prepare_creates_no_checkout_cache_and_reuses_matching_checkouts() {
    let root = TestDirectory::new("managed-clone");
    let remote = root.path().join("remote");
    std::fs::create_dir_all(remote.join("src")).unwrap();
    initialize_repository(&remote).await;
    write_file(&remote.join("src/value.txt"), "base\n");
    git(&remote, &["add", "."]).await;
    git(&remote, &["commit", "--quiet", "-m", "base"]).await;
    let base = git_stdout(&remote, &["rev-parse", "HEAD"]).await;
    git(&remote, &["tag", "review-base"]).await;
    let branch = git_stdout(&remote, &["symbolic-ref", "--short", "HEAD"]).await;
    write_file(&remote.join("src/value.txt"), "head\n");
    git(&remote, &["add", "."]).await;
    git(&remote, &["commit", "--quiet", "-m", "head"]).await;
    let head = git_stdout(&remote, &["rev-parse", "HEAD"]).await;
    let pull_request = pull_request(&remote, &branch, &base, &head);
    let cache = root.path().join("cache");
    let service = LocalRepositoryService::new_with_search_roots(
        ProcessRunner::new(),
        NoCredentialProvider,
        &cache,
        Vec::<PathBuf>::new(),
    );

    let managed = service.prepare(&pull_request, None).await.unwrap();
    assert!(!managed.reused_existing_checkout);
    assert!(service.is_managed_cache_path(&managed.path));
    assert!(
        !managed.path.join("src/value.txt").exists(),
        "managed clone must not populate a worktree"
    );
    let refs = git_stdout(
        &managed.path,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/pullrequest-scanner/review",
        ],
    )
    .await;
    assert_eq!(refs.lines().count(), 2, "expected review head/base refs");
    let ref_objects = git_stdout(
        &managed.path,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/pullrequest-scanner/review",
        ],
    )
    .await;
    assert!(ref_objects.lines().any(|object| object == head));
    assert!(ref_objects.lines().any(|object| object == base));

    let reused_cache = service.prepare(&pull_request, None).await.unwrap();
    assert_eq!(reused_cache.path, managed.path);
    assert!(!reused_cache.reused_existing_checkout);

    let search_root = root.path().join("search");
    std::fs::create_dir_all(&search_root).unwrap();
    let checkout = search_root.join("checkout");
    let remote_argument = std::path::absolute(&remote).unwrap();
    let checkout_argument = std::path::absolute(&checkout).unwrap();
    git(
        root.path(),
        &[
            "clone",
            "--quiet",
            remote_argument.to_string_lossy().as_ref(),
            checkout_argument.to_string_lossy().as_ref(),
        ],
    )
    .await;
    let service = LocalRepositoryService::new_with_search_roots(
        ProcessRunner::new(),
        NoCredentialProvider,
        root.path().join("other-cache"),
        [&search_root],
    );
    let reused_search = service.prepare(&pull_request, None).await.unwrap();
    assert_eq!(
        std::fs::canonicalize(&reused_search.path).unwrap(),
        std::fs::canonicalize(&checkout).unwrap()
    );
    assert!(reused_search.reused_existing_checkout);
}

#[tokio::test]
async fn prepare_explicit_checkout_never_changes_branch_index_or_worktree() {
    let root = TestDirectory::new("explicit-checkout");
    let remote = root.path().join("remote");
    std::fs::create_dir_all(remote.join("src")).unwrap();
    initialize_repository(&remote).await;
    write_file(&remote.join("src/value.txt"), "base\n");
    git(&remote, &["add", "."]).await;
    git(&remote, &["commit", "--quiet", "-m", "base"]).await;
    let base = git_stdout(&remote, &["rev-parse", "HEAD"]).await;
    git(&remote, &["tag", "review-base"]).await;
    let branch = git_stdout(&remote, &["symbolic-ref", "--short", "HEAD"]).await;
    write_file(&remote.join("src/value.txt"), "head\n");
    git(&remote, &["add", "."]).await;
    git(&remote, &["commit", "--quiet", "-m", "head"]).await;
    let head = git_stdout(&remote, &["rev-parse", "HEAD"]).await;
    let pull_request = pull_request(&remote, &branch, &base, &head);

    let checkout = root.path().join("checkout");
    let remote_argument = std::path::absolute(&remote).unwrap();
    let checkout_argument = std::path::absolute(&checkout).unwrap();
    git(
        root.path(),
        &[
            "clone",
            "--quiet",
            remote_argument.to_string_lossy().as_ref(),
            checkout_argument.to_string_lossy().as_ref(),
        ],
    )
    .await;
    write_file(&checkout.join("src/dirty.txt"), "keep me\n");
    let before_head = git_stdout(&checkout, &["rev-parse", "HEAD"]).await;
    let before_branch = git_stdout(&checkout, &["symbolic-ref", "--short", "HEAD"]).await;
    let before_status = git_stdout(&checkout, &["status", "--porcelain=v1", "-z"]).await;

    let service = LocalRepositoryService::new_with_search_roots(
        ProcessRunner::new(),
        NoCredentialProvider,
        root.path().join("cache"),
        Vec::<PathBuf>::new(),
    );
    let local = service
        .prepare(&pull_request, Some(checkout.join("src").as_path()))
        .await
        .unwrap();
    assert_eq!(
        std::fs::canonicalize(&local.path).unwrap(),
        std::fs::canonicalize(&checkout).unwrap()
    );
    assert!(local.reused_existing_checkout);
    assert_eq!(
        git_stdout(&checkout, &["rev-parse", "HEAD"]).await,
        before_head
    );
    assert_eq!(
        git_stdout(&checkout, &["symbolic-ref", "--short", "HEAD"],).await,
        before_branch
    );
    assert_eq!(
        git_stdout(&checkout, &["status", "--porcelain=v1", "-z"]).await,
        before_status
    );
    assert_eq!(
        std::fs::read_to_string(checkout.join("src/dirty.txt")).unwrap(),
        "keep me\n"
    );
}

#[tokio::test]
async fn explicit_checkout_requires_matching_normalized_origin() {
    let root = TestDirectory::new("origin-mismatch");
    let first = root.path().join("first");
    let second = root.path().join("second");
    initialize_bareless_repository(&first).await;
    initialize_bareless_repository(&second).await;
    git(
        &first,
        &["remote", "add", "origin", "https://github.com/one/repo.git"],
    )
    .await;
    let commit = git_stdout(&first, &["rev-parse", "HEAD"]).await;
    let mut pull_request = pull_request(&second, "master", &commit, &commit);
    pull_request.clone_url = "https://github.com/two/repo.git".to_string();
    pull_request.source_fetch_ref = format!("refs/{commit}");
    pull_request.target_fetch_ref = format!("refs/{commit}");
    let service = LocalRepositoryService::new_with_search_roots(
        ProcessRunner::new(),
        NoCredentialProvider,
        root.path().join("cache"),
        Vec::<PathBuf>::new(),
    );

    let error = service
        .prepare(&pull_request, Some(&first))
        .await
        .unwrap_err();
    assert!(!error.is_authentication());
    assert!(
        error
            .to_string()
            .contains("does not use the pull request repository")
    );
}

async fn initialize_bareless_repository(repository: &Path) {
    std::fs::create_dir_all(repository).unwrap();
    initialize_repository(repository).await;
    write_file(&repository.join("file.txt"), "initial\n");
    git(repository, &["add", "."]).await;
    git(repository, &["commit", "--quiet", "-m", "initial"]).await;
}

async fn initialize_repository(repository: &Path) {
    std::fs::create_dir_all(repository).unwrap();
    git(repository, &["init", "--quiet"]).await;
    git(repository, &["config", "core.autocrlf", "false"]).await;
    git(repository, &["config", "core.safecrlf", "false"]).await;
    git(
        repository,
        &["config", "user.email", "tests@example.invalid"],
    )
    .await;
    git(
        repository,
        &["config", "user.name", "PullRequest Scanner Tests"],
    )
    .await;
}

async fn git(repository: &Path, arguments: &[&str]) -> ProcessResult {
    let result = ProcessRunner::new()
        .run_async(
            "git",
            arguments.iter().copied(),
            Some(repository),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        result.succeeded(),
        "git {} failed: {}",
        arguments.join(" "),
        result.standard_error
    );
    result
}

async fn git_stdout(repository: &Path, arguments: &[&str]) -> String {
    git(repository, arguments)
        .await
        .standard_output
        .trim()
        .to_string()
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn review_state(repository: &Path, base: &str, head: &str) -> ReviewSessionState {
    ReviewSessionState {
        local_repository_path: Some(repository.to_string_lossy().into_owned()),
        details: PullRequestDetails {
            reference: PullRequestReference {
                provider: ReviewProvider::GitHub,
                repository: "test".to_string(),
                ..Default::default()
            },
            base_commit_sha: base.to_string(),
            head_commit_sha: head.to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn pull_request(remote: &Path, branch: &str, base: &str, head: &str) -> PullRequestDetails {
    let clone_url = std::path::absolute(remote)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    PullRequestDetails {
        reference: PullRequestReference {
            provider: ReviewProvider::GitHub,
            clone_url: clone_url.clone(),
            session_key: "github:local/prs/review#7".to_string(),
            ..Default::default()
        },
        source_fetch_ref: format!("refs/heads/{branch}"),
        target_fetch_ref: "refs/tags/review-base".to_string(),
        base_commit_sha: base.to_string(),
        head_commit_sha: head.to_string(),
        clone_url,
        ..Default::default()
    }
}
