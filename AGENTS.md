# Agent guide

## Purpose

PullRequest Scanner is a local-first WinUI 3 desktop app for reviewing GitHub
and Azure DevOps pull requests with an Azure DevOps-style review workflow.

Before editing a component, read `docs/architecture.md`. Preserve these
invariants:

1. Opening, refreshing, and reviewing a PR must never checkout, reset, clean,
   or otherwise modify an existing user worktree.
2. Access tokens are resolved for the current process only. Never persist them,
   place them on command lines, or include them in review state/logs.
3. Notes and queued comments remain local until an explicit publish action.
4. Provider-specific behavior stays behind `IReviewProviderClient`.
5. User-facing strings use `.resw` resources in C#, the compile-time resource
   catalog in Rust, and theme-dependent XAML uses `ThemeResource`.

## Repository map

| Path | Responsibility |
|---|---|
| `src/PullRequestScanner.Core` | Provider-neutral parsing, Git, credentials, persistence, Markdown, and navigation |
| `src/PullRequestScanner.App` | WinUI shell, MVVM, XAML, resources, packaging |
| `tests/PullRequestScanner.Core.Tests` | Provider/parser/persistence/Git behavior tests |
| `rust/crates/prs-core` | Rust provider-neutral Git, parsing, and service logic |
| `rust/crates/prs-app` | Rust native WinUI shell and pure app state |
| `docs/architecture.md` | Architecture, boundaries, privacy, and UI composition |

## Build and test

Run commands from the repository root on Windows:

```powershell
dotnet restore .\PullRequestScanner.sln --configfile .\NuGet.config
dotnet format .\PullRequestScanner.sln --no-restore --verify-no-changes
dotnet test .\tests\PullRequestScanner.Core.Tests\PullRequestScanner.Core.Tests.csproj -c Release --no-restore
dotnet build .\src\PullRequestScanner.App\PullRequestScanner.App.csproj -c Debug -r win-x64 --no-restore
dotnet build .\src\PullRequestScanner.App\PullRequestScanner.App.csproj -c Release -r win-x64 --no-restore
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p prs-app --release --target x86_64-pc-windows-msvc
cargo build -p prs-app --release --target aarch64-pc-windows-msvc
```

Run the smallest relevant test set while iterating, then run all tests and both
app implementations before handing off a change. Run
`pwsh .\rust\scripts\package.ps1 -Architecture all` for packaging or manifest
changes.

## Change conventions

- Keep provider-neutral logic in Core; do not introduce WinUI dependencies
  there.
- Keep XAML state declarative. Use view-model properties and commands instead
  of setting colors, layout, or business state imperatively in code-behind.
- Prefer `x:Bind Mode=OneWay` for active-review data so switching reviews does
  not retain stale bindings.
- Use WinUI theme dictionaries and semantic resource names. Update Light,
  Dark, and HighContrast together.
- Keep code rows virtualized and selection stable; avoid control replacement
  on row selection.
- Add focused tests for behavior changes. Do not add tests that only restate
  framework behavior.
- Update `docs/architecture.md` in the same change when an invariant or
  component boundary changes.

## Security and data handling

- Never commit tokens, generated review JSON, repository clones, package
  outputs, screenshots containing private PR content, or `%LOCALAPPDATA%`
  content.
- Use the existing credential boundary and process-scoped Git configuration.
- Do not render provider Markdown in a WebView. Use the existing safe parser.
- Keep public examples generic (`contoso`, `octo-org`); do not add internal
  hosts, employee names, paths, PRs, or work-item IDs.

## Definition of done

A change is ready when:

- The requested behavior is implemented end to end.
- Related docs/resources/manifest entries are updated.
- Formatting verification passes.
- All 66+ core tests pass.
- Debug and Release x64 app builds have zero warnings and errors.
- Rust formatting, Clippy, all workspace tests, and x64/ARM64 release builds pass.
- Public diffs contain no credentials, local paths, private PR data, or
  generated outputs.
