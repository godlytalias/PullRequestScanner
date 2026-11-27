# PullRequest Scanner architecture

## Goals

PullRequest Scanner is a packaged WinUI 3 application with a local-first review
model. Its core invariants are:

1. Opening or refreshing a review must never reset, checkout, clean, or modify
   files in an existing user's worktree.
2. Access tokens must not be persisted, placed on Git command lines, or included
   in review JSON.
3. Notes and queued comments must survive application restarts and must publish
   only after an explicit command. Explicit review completion removes that
   private state while retaining aggregate, non-secret activity metrics.
4. GitHub and Azure DevOps provider differences remain behind a common
   provider contract.
5. Static user-facing UI text is resource-backed (`.resw` in the C# app and the
   compile-time catalog in the Rust app). Theme-dependent presentation follows
   native WinUI theme colors; the C# XAML dictionaries use `ThemeResource`.

## Projects

```text
PullRequestScanner/
  Cargo.toml
  rust/
    crates/
      prs-model/                 C#-JSON-compatible models
      prs-core/                  provider-neutral Git, parsing, and services
      prs-providers/             GitHub and Azure DevOps clients
      prs-store/                 atomic review/activity persistence
      prs-winui-projection/      boundary over the published Naui WinUI projection
      prs-app/                   native Rust WinUI shell and app state
  src/
    PullRequestScanner.Core/    provider-neutral services and models
    PullRequestScanner.App/     WinUI shell, MVVM, resources, and packaging
  tests/
    PullRequestScanner.Core.Tests/
  docs/
    architecture.md
```

`PullRequestScanner.Core` targets .NET 8 and has no WinUI dependency. This keeps
URL parsing, Git operations, diff parsing, provider payloads, and persistence
testable on non-UI threads. `PullRequestScanner.App` is the packaged WinUI 3
composition root and remains the behavioral reference while the Rust
implementation completes its release cutover.

The Rust workspace has the same provider-neutral boundary. `prs-model` keeps the
JSON schema compatible with the existing C# state. `prs-core`, `prs-providers`,
and `prs-store` contain the Git/process, HTTP, and atomic-persistence logic.
`prs-app` owns UI-independent `MainAppState` and `ReviewSessionAppState` values,
then renders them through raw WinUI 3 controls loaded from the C#-equivalent XAML
under `prs-app/resources`. It does not depend on preview `windows-reactor`, and it
does not run a local binding generator. `prs-winui-projection` reexports the
committed `naui-winui3` projection used for Naui's supported runtime boundary and
native hosts.

The native shell uses a Tokio runtime for provider/Git/storage work and Naui
channels to marshal all control updates back to the STA UI thread. Naui remains
the application/window/STA bootstrap, channel adapter, and host for the native
virtualized list, file tree, drawing surfaces, activity table, and invisible
file-picker engine. The visible shell is not composed from Naui's generic
`Toolbar`, `Sidebar`, `SplitView`,
or `Tabs` presentation controls; the only Naui table remains the contained
activity-history surface. `ui/raw.rs` resolves and updates named controls from
raw WinUI XAML (including mounting the single title-bar **Open PR** action into
Naui's native 32-DIP Mica caption row with drag-region exclusion), while
`ui/native.rs` wraps only the native virtualization, `ContentControl` pane
splitters, `NavigationView` compact-footer chrome, and `DiffList` container
padding and side-by-side viewport width synchronization. Standalone item
templates carry their own theme/style resources so `XamlReader` can realize
them outside the shell's local resource scope.

Native WinUI callbacks remain on the UI thread; only serializable state and safe
error text cross the channel. The Windows text-selection notification is
routed through a Naui channel so selecting a raw `TextBlock` can create a
persisted character-range comment without replacing the source row or changing
its measured height. Programmatic `NavigationView` selection is idempotent in
the controller to prevent `SelectionChanged` from recursively rebuilding and
reselecting the active item. The controller has process-lifetime ownership so
WinRT callbacks and weak widget references are never dropped during thread-local
shutdown.

## Opening a review

1. `PullRequestUrlParser` identifies GitHub/GitHub Enterprise or Azure DevOps
   and creates a stable session key.
2. `IReviewProviderClient` loads PR metadata, source/target commit IDs, and
   review iterations.
3. `CliCredentialProvider` resolves API credentials from a process environment
   variable or the authenticated `gh`/`az` CLI. Anonymous GitHub access remains
   available for public repositories.
   Unauthorized provider responses become a typed
   `ReviewAuthenticationException`. The UI offers an interactive `gh auth
   login --web` or `az login --use-device-code` flow, clears rejected
   environment credentials from that child process, verifies a CLI credential,
   and retries the original request.
   GitHub API calls use Bearer authorization, while clone/fetch converts the same
   token to the Basic `x-access-token` form required by Git smart-HTTP.
   A rejected Git credential produces `ReviewAuthenticationException`; the app
   does not silently downgrade to an anonymous transport.
4. `LocalRepositoryService` checks an explicit path, the app cache, and
   configured search roots. A candidate is reusable only when normalized
   `origin` matches the PR's base repository.
5. The service fetches source and target refs into `refs/pullrequest-scanner/review/*`; it
   never changes the current branch or index. Git credentials are injected as
   a host-scoped `http.extraHeader` in the child process environment.
6. `GitDiffService` derives changed-file status and renders diffs directly from
   the Git object database.
7. The provider loads review-line, conversation, and review-summary comments so
   the reviewer can see existing feedback.
8. `JsonReviewStateStore` atomically updates the session state.

## Review state and privacy

Each PR owns one JSON file under app-local `Reviews`. `ReviewSessionState`
contains:

- Provider identity, URL, branches, and commit IDs.
- The selected repository path and whether it was an existing checkout.
- Changed file paths and viewed-file state.
- Provider iterations, selected iteration, and last-reviewed iteration IDs.
- Whether a review was manually archived in local history.
- Private notes.
- Queued, published, and failed comment records.
- A persisted snapshot of comments already present on the pull request.
- Remote comment thread and parent IDs for reply grouping.
- The PR description, selected iteration, and selected file at schema version 5.
- An optional completion timestamp.

Writes use a temporary sibling file followed by overwrite move. Failed comment
records preserve the provider error and remain queued for retry.

Activity is stored separately under `Activity/review-activity.json`. It records
open, publish, reply, reviewed, and completion events without credentials or
comment bodies. Completing a review deletes the per-review state file and its
managed cache checkout only when the cache is not shared by another saved
review. Explicit user checkouts are never deleted. Aggregate activity remains
available to the monthly dashboard after cleanup.

Access tokens never enter this model. A host-scoped Git configuration entry is
constructed only for the lifetime of the child process. State is stored under
`%LOCALAPPDATA%\PullRequestScanner`; it is local but not encrypted.

## Diff and definition model

`GitDiffService` uses:

- `git diff --name-status -z` for the changed-file tree.
- `git diff --unified=<N>` for folder hunks.
- A very large unified context for a complete-file view.
- `git show <commit>:<path>` to render a definition target.

`UnifiedDiffParser` preserves old/new line numbers and classifies each line as
added, deleted, context, or hunk. The line side and original number are the
source of truth when creating provider review comments.

`DiffSideBySideBuilder` pairs each contiguous deletion group with the following
addition group by ordinal position. Context lines exist on both sides and hunk
headers remain full-width. The C# UI keeps independent inline and side-by-side
virtualized lists so changing layout cannot destabilize `ItemsSource` identity.
Rust projects the same immutable row states through one native list and switches
the raw `DiffLine`/`SideBySideRow` builders without coupling state to realized
containers.

GitHub represents each PR commit as an iteration; the commit's parent is the
iteration base. Azure DevOps uses the provider's iteration snapshots, using the
previous iteration's source commit as the delta base and the first iteration's
common/target commit for the initial comparison. An **All changes** synthetic
range always preserves the original PR base-to-head view. Diff, file listing,
definition lookup, and comments use the selected range without changing the
user's checkout.

Definition navigation extracts callable identifiers from each line, preserves
qualification for display, and suppresses known external roots such as `std`
and `System`. `SymbolCandidate.ColumnIndex` is retained so the definition link
is rendered beneath the callable with a bounded monospace indentation rather
than in a far-right column.
`SymbolNavigationService` searches Git objects with exact word and language path
constraints, then accepts only declaration-shaped matches. A call-site-only
search returns no result rather than navigating to the original line. The
search no longer limits the first match in each file, so later declarations
remain discoverable. Deleted lines search and render from the base commit;
other lines use the head commit. This is a heuristic, not full semantic
overload resolution.

## Provider publishing

The common `IReviewProviderClient` contract exposes PR loading, iteration
loading, paginated existing-comment loading, one-comment publishing, comment
deletion, and thread replies.

- GitHub uses the REST pull-request and review-comment endpoints.
- Azure DevOps uses the Git REST pull-request and thread endpoints.

Queued comments reference a file, provider side, and line number. They can also
store a selected character range, selected text, and a code replacement. GitHub
publishes replacements as a native suggestion block on the right side; ADO
publishes an explicit Markdown diff because its thread API has no equivalent
native code-suggestion block.

Publishing is serialized and each successful remote ID is persisted
immediately. A failure on one comment does not discard remaining local comments
or imply success. Existing comments are deduplicated against locally published
comments by remote ID. If publishing returns 401/403, the comment stays failed,
the global sign-in banner is shown, and its pending comments are retried after
authentication. Publishing always uses the provider API credential and never
uses Git's anonymous/public-read transport state.

## WinUI composition

`MainViewModel` owns active and reviewed-pull-request collections and updates
their ordering incrementally so NavigationView selection keeps stable object
identity. Startup restores the local snapshot, auto-selects the last or first
file, and refreshes summary metadata. Every five minutes,
active reviews perform a full refresh of PR metadata, iterations, changed
files, comments, and the selected diff. Completed, abandoned, closed,
or manually archived reviews move to the reviewed collection without deleting
local notes or comments. Reopening a non-terminal archived PR returns it to the
active collection. The separately confirmed **Complete and clean up** command
removes the review from local state, deletes an unshared managed cache, protects
explicit checkouts, and records aggregate completion activity.

Each `ReviewSessionViewModel` owns one PR's:

- File hierarchy and selected folder/file.
- Diff/context and symbol-navigation state.
- Viewed-file state, file filtering, and pane-width preferences.
- Notes and local comment draft.
- Inspector tab/filter state and a debounced note-saving status.
- Expandable remote comment threads and reply drafts.
- Character selection and optional code-suggestion draft.
- Selected iteration and explicit last-reviewed iteration tracking.
- Safe parsed Markdown blocks for PR descriptions and `.md` files.
- Publish, refresh, and save commands.

The C# composition root is `MainPage.xaml`; the Rust composition root is the
equivalent `Shell.xaml` loaded by `XamlReader`. Both provide the same
`NavigationView`, compact file tree, virtualized diff surface, `CommandBar`,
and segmented notes/comment inspector. Rust binds state imperatively by named
control because compiled `{x:Bind}` cannot target its Rust state objects, while
the raw `DiffLine`, side-by-side row, navigation item, and comment/reply
templates retain the C# geometry and theme resources. Single-child folder
chains are projected as one `path/to/folder` node while each leaf retains its
full file path. The C# app uses MVVM Toolkit partial observable properties for
AOT-safe WinRT marshalling.
The repository path may be entered directly or populated by a WinUI folder
picker initialized against the main window handle.
Normal-mode code rows use persistent selectable text assembled from
`DiffLineViewModel` display runs, so selecting a row does not swap controls or
change line height. Focused-mode commentable lines remain full-width line
buttons. A comment thread
card is rendered under the exact old/new diff line without a detached pointer.
Comment save/publish controls
bind their `IsEnabled` state to explicit readiness properties and notify on
line, draft, busy, and pending-comment changes. Active-review commands use
`x:Bind Mode=OneWay` so switching PRs cannot leave controls bound to the
previous review.
Inline and inspector comments use stretch containers and wrapped text. Existing
HTML/Markdown is flattened to safe plain text before display; remote
`ReviewCommentViewModel` exposes either an in-app line navigation command or a
provider URL, groups replies beneath their root thread, and supports explicit
collapse/expand, lazy reply composition, and local-draft editing; line comments are selected and scrolled after all virtualized
rows have been rebuilt. A short retry loop waits for the row container to
realize before calling `StartBringIntoView`.
Selecting a file loads the complete diff and scrolls the first changed line
into the viewport; previous/next arrow controls traverse changed lines.
Definition
navigation uses a leading-edge scroll request so the declaration appears at the
top of the diff surface and exposes an explicit **Back to diff** command.
Definition-mode lines are not commentable, and symbol extraction strips comments
and strings before offering a single compact peek action.
The active navigation list leads with the PR number and title, then the
repository. Native `SelectionChanged` and `ItemInvoked` events converge on the
same selection command, and the shell assigns only `NavigationView.SelectedItem`.
It does not directly mutate individual realized item containers, avoiding
recycled-container races on pointer clicks. `LastOpenedUtc` changes only when a
review is explicitly opened or reopened, not on ordinary state saves, so pointer
targets cannot reorder while a click is in flight.
The pane footer exposes merged/archived PR history and monthly activity.
Diff actions live in a `CommandBar` with secondary overflow, while progress uses
its own fixed-height row so resizing cannot overlap the toolbar or code. Pane
splitters use a small `PaneSplitter` control that supplies the WinUI
`SizeWestEast` cursor, captures the handled pointer stream at the page level,
releases pointer capture only for active splitter drags, and remains
keyboard-resizable with the same view-model width commands.

The normal opening form is collapsed during an active review and can be opened
from the title bar. The activity dashboard stays inside the NavigationView
shell, so the review and five-minute timer remain alive. `MainPage` synchronizes
the review workspace and embedded activity page visibility explicitly from the
view-model state, suppresses `SelectionChanged` while the dashboard is active,
rejects review-item invocation/tap callbacks until Back is used, and clears the
open-review selection synchronously before layout. The page remains cached in
the shell. The code workspace can
expand to show a compact navigation rail while hiding the file and inspector panes.
In that mode, commentable code lines become line-shaped buttons so pointer,
touch, and keyboard activation all open the existing comment composer. `.md` files
can switch between the virtualized raw diff and a safe block-based formatted
view; formatted blocks retain line-anchored commenting. PR descriptions use the
same parser in a dialog. Description, activity, Markdown, thread, and completion
actions use visible text labels so they remain discoverable. Both content
dialogs are part of the page visual tree rather than `Page.Resources`, which
keeps their `Popup` host populated on packaged WinUI. Markdown is never rendered
in an unsanitized WebView.

`ReviewActivityPage` reads the retained aggregate store, renders daily PR and
comment bars for one month, and supports month navigation. The monthly activity
data contains no review notes, code, or comment bodies.

## Rust packaging and release

`cargo build -p prs-app` uses the framework Windows App SDK already installed
on the development machine. Packaging enables the `prs-app/self-contained`
feature, which activates Naui's published self-contained backend. Its
`naui-winappsdk` bundle contains the Windows App SDK DLLs, activation manifest,
 resources, and license notices beside the executable. `rust/scripts/package.ps1`
 stages that bundle with the existing application assets and
`rust/package/AppxManifest.template.xml`, then uses the published WinApp CLI to
emit architecture-specific unsigned MSIX files. The script uses only
hash-verified NuGet archives; offline builds can copy the Naui component
archives from the local NuGet cache into `target/naui-winappsdk-packages`.
The Rust package retains the C# package identity so the signed output replaces
the existing app rather than registering a second product.

Release validation builds both `x86_64-pc-windows-msvc` and
`aarch64-pc-windows-msvc`. The HTTP stack uses Windows native TLS rather than a C
crypto provider, so the ARM64 build does not require a separate Clang toolchain.
The package contains no token, local review JSON, repository cache, certificate,
or development state.

The compact NavigationView menu is a single-row 48 DIP `NavigationViewItem`
template with no nested menu item. It contains the PR number/title and a
single-line repository subtitle, while the tooltip retains the full title.
The menu, file tree, and diff list expose meaningful UIA names rather than raw
.NET type names.

Theme additions use complete Light/Dark/HighContrast dictionaries. The toolbar
spans both header columns, progress is shown beneath it, and the
file/inspector panes have persisted thumb splitters. Code rows use Auto line
number columns in side-by-side mode, shared horizontal scrolling, semantic
addition/deletion strokes, and substring/intraline highlights. The overview
minimap sits beside the scroll viewport with 24 DIP targets and is recomputed
from visible item positions after folding.

HighContrast variants use system color brushes and the app disables double
High Contrast adjustment.

## Extension points

Additions should preserve the boundaries:

- Add a provider by implementing `IReviewProviderClient` and extending
  `PullRequestUrlParser`.
- Add a local index by replacing or supplementing
  `SymbolNavigationService`; do not move language-specific logic into XAML.
- Persist schema changes monotonically with `SchemaVersion` migration in
  `JsonReviewStateStore`.
- Add settings through the app composition root; never put secret material in a
  persisted view model.

Before changing a component in this folder, read this document and ensure the
change preserves the five invariants above.
