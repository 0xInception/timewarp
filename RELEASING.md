# Releasing

```sh
scripts/release.sh 0.2.0
```

Add a `## 0.2.0 (unreleased)` section to CHANGELOG.md first. The script bumps
the version, dates the changelog, commits, tags `v0.2.0` and pushes. GitHub
Actions builds Timewarp.app for Apple silicon and Intel, then waits for you to
approve the `release` environment. Approving signs `latest.json` and publishes
the release, and running copies of Timewarp offer it within a day.

## Why only you can ship an update

The app only installs a release whose `latest.json` is signed by the release
key, and only if every download matches the size and SHA-256 listed in it.
The public key is built into the app (`apps/desktop/update-key.pub`). The
private key lives in two places: your machine and the `release` environment
secret. Someone who pushes to the repo, or even publishes a GitHub release,
still can't make Timewarp install anything.

## One-time setup

1. Create the GitHub repository. Set `repository` in `Cargo.toml` to its URL,
   because the app looks for updates there. Push `main`.
2. Make the key:
   ```sh
   cargo run -p timewarp-release -- keygen
   ```
   Commit `apps/desktop/update-key.pub`. Save
   `~/.config/timewarp/release-key` in your password manager. If you lose it,
   installed copies can't update any more and have to be reinstalled by hand.
3. Settings → Environments → New environment `release`:
   - Required reviewers: you.
   - Deployment branches and tags: Selected, add the tag rule `v*`.
   - Add the secret:
     `gh secret set TIMEWARP_UPDATE_KEY --env release < ~/.config/timewarp/release-key`
4. Settings → Rules → Rulesets → New tag ruleset: target `v*`, restrict
   creations, updates and deletions, and leave only yourself on the bypass list.
   Add a branch ruleset for `main` that blocks force pushes and deletion.

## Optional: Developer ID

Without one the app is signed ad hoc. It runs fine, but macOS asks for Screen
Recording permission again after every update, because the signature changes.
With an Apple Developer ID, add these repository secrets and the workflow
signs and notarizes the app:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE` | Developer ID Application certificate, `.p12`, base64 |
| `MACOS_CERTIFICATE_PASSWORD` | its password |
| `MACOS_SIGNING_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | for notarization |

Once a release has a team signature, updates must come from the same team.
