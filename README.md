# PullRequest Scanner

PullRequest Scanner is a local-first WinUI 3 desktop application for reviewing
GitHub and Azure DevOps pull requests.

## Features

- Opens GitHub.com, GitHub Enterprise, and Azure DevOps pull request URLs.
- Keeps multiple reviews in the navigation pane and restores saved reviews on
  startup. Active entries show the PR title; merged/closed/manual-archive
  entries remain available under **Reviewed pull requests**.
- Selects **All changes** or an individual iteration. GitHub uses PR commits as
  iterations; Azure DevOps uses native PR iteration snapshots.
- Marks the latest iteration reviewed and shows a navigation pill when a newer
  iteration appears. Active PR metadata refreshes on startup and every five
  minutes.
- Reuses a matching checkout from `%USERPROFILE%\repos` or `%USERPROFILE%\src`;
  an existing path can be typed or selected with the repository folder picker.
- Fetches PR commits into review refs without checking out, resetting, or
  cleaning a user's working tree.
- Shows a hierarchical changed-file tree. File selection renders the complete
  unified diff; folder selection renders changed hunks with configurable
  context.
- Switches between **Inline** and **Side by side** diff layouts. Side-by-side
  rows pair deleted and added lines while keeping hunk and context rows aligned.
- Distinguishes additions and deletions with theme-aware surfaces and supports
  clickable callable-name links to the best-ranked declaration.
- Places definition links beneath the callable at its character-column offset
  rather than at the right edge of the code viewport.
- Shows local and existing provider comments in balloons directly beneath the
  exact old/new diff line. Balloon and inspector text wraps to the available
  pane width.
- Clicking **Existing** on an inline review comment jumps to its exact line;
  a conversation-level comment opens its provider URL.
- Selects a character range in a diff line for a precisely quoted local comment.
- Shows locally saved and existing provider comments as inline balloons, plus a
  complete list of existing PR comments in the review inspector.
- Saves optional suggested replacement text. GitHub publishes native
  `suggestion` blocks; Azure DevOps receives an equivalent Markdown diff.
- Uses previous/next arrow controls to move through changed lines, opens a
  complete file at its first change, and aligns navigated definitions to the
  top of the diff view.
- Saves private notes and queued line comments in local JSON state.
- Publishes queued comments to GitHub or Azure DevOps only when
  **Publish pending comments** is selected.

## Authentication

The app does not store access tokens. API and Git requests resolve credentials
in this order:

### GitHub

1. `GH_TOKEN`, `GITHUB_TOKEN`, or `PULLREQUEST_SCANNER_GITHUB_TOKEN`.
2. `gh auth token --hostname <host>`.

Authenticate with:

```powershell
gh auth login --hostname github.com
```

Run `gh auth setup-git` if Git Credential Manager is not already configured for
the host.

### Azure DevOps

1. `AZURE_DEVOPS_EXT_PAT`, `PULLREQUEST_SCANNER_AZURE_DEVOPS_PAT`, or
   `SYSTEM_ACCESSTOKEN`.
2. `az account get-access-token` for the Azure DevOps resource.

Authenticate with:

```powershell
az login
```

When a provider rejects a request as unauthorized, the app opens a **Sign in
required** banner. **Sign in and retry** launches the appropriate interactive
`gh` or `az` flow, waits for it to finish, and retries the original pull
request. Environment tokens that triggered the rejection are ignored for the
rest of that app session so a newly authenticated CLI credential can be used.

Credentials are passed to API requests as authorization headers and to Git in a
short-lived process environment entry scoped to the repository host. They are
not written to review state, command lines, or application logs.

GitHub's API and Git transports use different accepted formats: the app uses a
Bearer token for REST API requests and a GitHub-compatible Basic smart-HTTP
header for clone/fetch. A Git authentication failure is never treated as
anonymous success. It opens the sign-in flow, while comment publishing remains
an authenticated API operation and retries the pending comments after sign-in.

## Repository discovery

The app checks these locations for an exact, origin-matched repository:

1. The optional path entered for the review.
2. The app cache created for that repository.
3. Immediate child directories under `%USERPROFILE%\repos`.
4. Immediate child directories under `%USERPROFILE%\src`.

Additional roots can be supplied as a semicolon-separated
`PULLREQUEST_SCANNER_REPO_ROOTS` environment variable. If no match exists, the
repository is cloned with `--filter=blob:none --no-checkout` into application
storage, with a normal no-checkout clone fallback for servers that do not
support partial clone.

## Build

Requirements:

- Windows 10 1809 or later for the current package manifest; a current Windows 11
  SDK/WinUI runtime is recommended.
- .NET 8 SDK support from a .NET 10 or newer SDK command line.
- Git.

```powershell
dotnet restore .\PullRequestScanner.sln --configfile .\NuGet.config
dotnet build .\PullRequestScanner.sln --no-restore
dotnet test .\tests\PullRequestScanner.Core.Tests\PullRequestScanner.Core.Tests.csproj --no-restore
```

Run from Visual Studio with the package project as the startup project, or use:

```powershell
dotnet run --project .\src\PullRequestScanner.App\PullRequestScanner.App.csproj --no-restore
```

The repository's `NuGet.config` restores only from public NuGet.org.

## Development

- Start with [AGENTS.md](AGENTS.md) for the component map, validation commands,
  security invariants, and agent handoff contract.
- Read [docs/architecture.md](docs/architecture.md) before changing component
  boundaries or persistence behavior.
- Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Local state

Review state is stored under `%LOCALAPPDATA%\PullRequestScanner`:

- `Reviews`: one atomic JSON file per PR, containing PR metadata, notes, viewed
  files, and local/published comment state.
- `Repositories`: no-checkout Git caches for repositories that were not reused
  from an existing local checkout.

Notes and queued comments are private local data. They are not encrypted and
should not be treated as a replacement for an approved secrets store.

## Symbol navigation

Navigation is backed by Git objects rather than a language server. Callable
names on a selected line are searched from the current file into its directory
and language-related path specs, then ranked toward declaration-shaped lines.
Known standard-library qualifications such as `std::clamp` are not offered as
repository-navigation links. If the checkout contains only call sites and no
declaration, navigation reports that no definition was found instead of
returning to the call line.
It is intentionally provider-independent, but language-specific semantic
resolution (overloads, generated types, macros, and dynamic dispatch) remains
an area for a future compiler/index integration.

See [docs/architecture.md](docs/architecture.md) for component boundaries and
security invariants.

## License

No license has been declared for this repository. Public visibility does not
by itself grant permission to redistribute or reuse the source.
