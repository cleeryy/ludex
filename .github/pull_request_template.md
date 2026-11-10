## Summary

<!-- What changes, and why? Link the issue if there is one. -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor / internal
- [ ] Documentation
- [ ] CI / build / packaging
- [ ] Security

## Testing

<!-- Paste commands and results, not just "tested". -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace --all-targets`
- [ ] `pnpm --filter @ludex/web build`
- [ ] `pnpm --filter @ludex/web exec tsc --noEmit`
- [ ] `scripts/e2e.sh` (or explain why not)
- [ ] Desktop check/build if `apps/desktop` changed

## Checklist

- [ ] No secrets, tokens, personal data or course PDFs in the diff.
- [ ] Database migrations are additive and forward-only.
- [ ] Public API changes are reflected in `docs/CONTRACT.md`.
- [ ] User-facing changes are in `CHANGELOG.md`.
- [ ] I did not edit generated files or commit build output.
- [ ] I have read `CONTRIBUTING.md`.

## Screenshots / recordings

<!-- For UI changes. Use the abstract mockups or real screenshots, no game keys. -->
