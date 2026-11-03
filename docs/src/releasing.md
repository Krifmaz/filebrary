# Releasing

Releases are tagged by CI. Nobody needs to push a tag by hand, which also
means release tagging works from tools that can push branches but not tags.

## Cutting a release

1. Make sure the workspace version in the root `Cargo.toml`
   (`[workspace.package] version`) is the version you are releasing, and that
   CI is green on the commit.
2. Push a branch named after the tag, pointing at that commit:

   ```sh
   git push origin <commit>:refs/heads/release/v0.2.0
   ```

   Pre-releases work the same way: `release/v0.2.0-rc.1` creates the tag
   `v0.2.0-rc.1`. The Cargo version may be `0.2.0` or `0.2.0-rc.1`.
3. The **Tag release** workflow (`.github/workflows/tag-release.yml`) then:
   * checks that the branch name is a version (`vX.Y.Z` or `vX.Y.Z-suffix`);
   * checks that `Cargo.toml` at the branch head has the same version;
   * creates the tag on the branch head commit;
   * runs the **Release** workflow for that tag, which builds the binaries
     for every platform, publishes the GitHub release with checksums, and
     pushes the Docker images (`X.Y.Z`, `X.Y`, `latest`, and the same with
     `-full`) to `ghcr.io`. Pre-releases are marked as such and do not move
     `latest`.

The release branch can be deleted once the release is published; the tag
stays.

## Tagging an older commit

The push trigger uses the workflow files in the pushed commit, so it only
works for commits that already contain `tag-release.yml`. To tag an older
commit, run **Tag release** by hand from the Actions tab (or with
`gh workflow run tag-release.yml -f tag=v0.1.0 -f sha=09b23d9`). It takes the
tag and a commit SHA (short or full) or branch name.

GitHub only offers a manual run once it has seen the workflow run at least
once. If the dispatch answers "Not Found" in a fresh fork, push any branch
that starts with `release/v` but is not a version (for example
`release/v-register`) once. That run fails at the first check without tagging
anything, and the manual run is available after it. Delete the branch
afterwards.

## Rules and safety

* If the tag already exists on the same commit, the run does nothing, so
  pushing more commits to a release branch never moves or rebuilds a
  published release. To rebuild an existing tag, run the **Release**
  workflow by hand with that tag.
* If the tag exists on a different commit, the run fails. Released tags are
  never moved; bump the version and release again.
* Runs for the same tag are queued, never run in parallel.
* A tag created by a workflow with `GITHUB_TOKEN` does not start other
  workflows, which is why **Tag release** calls **Release** directly (as a
  reusable workflow) instead of relying on the tag push.
* Pushing a tag by hand still works: the **Release** workflow also runs on
  any `v*.*.*` tag push.
