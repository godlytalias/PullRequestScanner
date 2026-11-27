# Rust migration

## Decision

The Rust implementation lives alongside the working C# application until two
full parity passes and the packaging/release gates are complete. The C# app
remains the behavioral reference during that transition.

The UI does **not** use `windows-reactor`. It is preview-only. The Rust UI uses
the published `naui` Windows backend, which creates real WinUI 3 controls on top
of the published `naui-winui3` projection. No local WinMD generation or
`windows-bindgen` build script runs in this repository. The published crate
provides the generated metadata bindings, hand-written `Application`
composition, dispatcher, and `IWindowNative` pieces.

This path is manual and requires explicit tests for:

- packaged Windows App Runtime initialization;
- STA and dispatcher-queue ownership;
- WinUI activation, metadata, and interface inheritance;
- ContentDialog, NavigationView, virtualized lists, focus, IME, and shutdown;
- x64 and ARM64 MSIX packaging.

## Workspace

| Crate | Responsibility |
|---|---|
| `prs-model` | C#-JSON-compatible models and pure domain types |
| `prs-core` | URL parsing, Markdown, diff/fold/symbol logic, Git process, and services |
| `prs-providers` | GitHub and Azure DevOps clients |
| `prs-store` | Atomic review/activity JSON state stores |
| `prs-winui-projection` | Stable boundary over the published `naui-winui3` projection |
| `prs-app` | WinUI 3 native Rust shell and interaction state |

`prs-app` keeps pure `MainAppState` and `ReviewSessionAppState` values separate
from Naui controls. Tokio performs provider, process, and persistence work.
Naui channels deliver results to the STA UI thread, where controls are updated.
Static strings are compiled from `resources/en-US/messages.txt` so the app does
not introduce raw UI literals into the Rust control composition.

## Invariants

The Rust implementation must preserve the existing architecture guarantees:

1. Never reset, checkout, clean, or otherwise modify an existing user
   worktree.
2. Never persist credentials or put tokens on process command lines.
3. Persist notes/comments locally and publish only after explicit action.
4. Keep GitHub and Azure DevOps behavior behind a provider trait.
5. Keep user-facing strings resource-backed and maintain Light, Dark, and High
   Contrast presentation.

## Parity sequence

- [x] C# JSON schema and pure parsing tests.
- [x] Git/diff/persistence parity on fixture repositories.
- [x] Provider contract tests from captured JSON fixtures.
- [x] Native WinUI activation, navigation, dialogs, virtualized diff, comments,
  focus mode, and monthly activity.
- [x] x64 and ARM64 self-contained MSIX packaging.
- [x] Run two live interaction/regression passes (development and staged
  self-contained package), covering the ported workflows and shutdown.
- [ ] Match the C# visual composition: Mica/custom title bar, NavigationView
  menu templates and footer, CommandBar grouping/overflow, exact pane metrics and
  splitters, diff line templates/fonts/backgrounds, inspector cards, and
  theme/high-contrast dictionaries.
- [ ] Cut over the primary release/CI startup path and remove the C# app after
  visual and behavioral parity are complete.

## Build

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p prs-app --release --target x86_64-pc-windows-msvc
cargo build -p prs-app --release --target aarch64-pc-windows-msvc
pwsh .\rust\scripts\package.ps1 -Architecture all
```

Normal development builds use the installed Windows App SDK framework package.
Packaging enables the optional `self-contained` feature and copies Naui's
hash-verified `naui-winappsdk` directory beside the executable. Offline
packaging reuses the matching component `.nupkg` files from the local NuGet
cache; otherwise Naui obtains the published packages from NuGet. The package
keeps the C# app's identity so signing the Rust output performs a cutover rather
than installing a side-by-side app.

Provider and Git operations never embed credentials, local reviews, repository
contents, or private pull-request data in the executable or package.
