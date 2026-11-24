# Copilot instructions

- Read `AGENTS.md` and `docs/architecture.md` before editing.
- Keep the UI consistent with an Azure DevOps-style desktop review flow.
- Preserve local-first behavior: never mutate a user's existing checkout.
- Keep provider differences in Core behind `IReviewProviderClient`.
- Never persist or log tokens. Never commit local state, review payloads, or
  private PR screenshots.
- Use `.resw` strings and `ThemeResource` brushes for visible UI.
- Run the validation commands in `AGENTS.md`; targeted tests are useful while
  iterating, but the final gate is all tests plus Debug and Release app builds.
