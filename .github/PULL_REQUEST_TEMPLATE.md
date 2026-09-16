## Summary

## Validation

- [ ] `pnpm run verify`
- [ ] No real user data was deleted during testing
- [ ] Destructive-path changes include focused Rust tests
- [ ] UI changes include browser regression evidence when applicable

## Security and privacy

- [ ] No secrets, credentials, private paths, logs, or local app data are included
- [ ] IPC inputs remain bounded and backend-authorized
- [ ] Cleanup still revalidates immediately before deletion
- [ ] Privacy or local-storage behavior changes are documented
