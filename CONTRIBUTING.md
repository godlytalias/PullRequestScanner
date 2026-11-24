# Contributing

## Start here

Read `AGENTS.md` and `docs/architecture.md` before making changes. Open an
issue for substantial UI, provider, storage, or security changes so the design
can be agreed before implementation.

## Local workflow

1. Fork or clone the repository on Windows.
2. Install a current .NET 10+ SDK, Git, and the WinUI/Windows App SDK tooling
   required by Visual Studio.
3. Restore from the repository's `NuGet.config`.
4. Make a focused change and add targeted tests.
5. Run the formatting, test, and build commands in `AGENTS.md`.
6. Open a pull request using the template.

## Pull request expectations

- Explain the user-visible behavior and the affected component boundary.
- Include screenshots for visible UI changes.
- Do not commit tokens, private review content, local-state JSON, package
  outputs, or generated repository caches.
- Keep provider credentials and secret-bearing tests out of fixtures; use
  in-memory handlers and example hosts.
- Preserve the local-first review and no-worktree-mutation invariants.

## Reporting security issues

Do not post security vulnerabilities, tokens, or private PR content in a
public issue. Follow `SECURITY.md`.
