# Code signing policy

GSwitch is an independent open-source desktop application. This policy
describes the roles and release facts used for a possible SignPath Foundation
application. SignPath Foundation decides eligibility and approval externally;
this document is not a promise that an application will be accepted.

**Free code signing provided by SignPath.io, certificate by SignPath Foundation**

## Project facts

- Project: GSwitch
- Source repository: <https://github.com/ginbing/GSwitch>
- License: GNU Affero General Public License v3.0 (AGPL-3.0)
- Download page: <https://github.com/ginbing/GSwitch/releases>
- Release form: GitHub Releases containing a Windows current-user NSIS
  installer, macOS DMGs, and Linux AppImage/Debian packages.
- Build source: tagged repository commits built by the repository's GitHub
  Actions release workflow.
- Existing release: `v1.0.0` is published; the current synchronized product
  version is `1.0.1`.

The application is local-first. It has no analytics, telemetry, crash-reporting
service, listening server, or remote-control surface. The repository does not
currently contain a SignPath project identifier, artifact configuration, or
production signing integration.

## Signing roles

The current repository has one direct GitHub collaborator. The roles below
reflect the live repository permissions and must be updated if the project
team changes:

- **Authors, committers, and reviewers:** `squarepots`, the current direct
  collaborator with GitHub `admin`, `maintain`, `push`, `pull`, and `triage`
  permissions. Changes proposed by anyone without committer responsibility
  require review by this role before entering a release branch.
- **Approver:** `squarepots`, the current repository administrator and release
  maintainer. This role decides whether a specific release is ready for a
  future code-signing request.

These roles are repository roles, not invented legal or company identities.
SignPath and GitHub account MFA requirements remain external operational steps.

## Network and privacy

GSwitch contacts Codex/OpenAI only for account operations requested by the
user, such as sign-in, credential validation, quota, switching, reset-credit,
or Wake operations. It uses GitHub Releases to check for and download signed
application updates. It does not send analytics or telemetry.

## Distribution and uninstall

GitHub Releases remain the primary distribution path. The current Windows
installer is not Authenticode-signed; the Tauri updater signature is a separate
signature for update integrity. macOS remains ad-hoc signed, and Linux
distribution is unchanged. Uninstall instructions for each platform are kept
in the repository [README](../README.md).

Microsoft Store/MSIX is a separate, deferred distribution decision. This
repository does not add Store packaging or a second update path.

## Future signing order

If SignPath Foundation accepts the project and returns real configuration, a
Windows release must preserve this order:

1. Build the Windows bundle from the exact release commit.
2. Authenticode-sign the final Windows executable/installer through the
   approved SignPath trusted-build path.
3. Generate the Tauri updater signature from those final Authenticode-signed
   installer bytes.
4. Generate or update `latest.json` for that exact signed artifact.
5. Publish the reviewed draft release assets.

An updater signature must never be generated before Authenticode changes the
installer. Application, approval, MFA setup, project identifiers, and service
configuration are external TBD work and are intentionally not represented in
this repository yet.

The policy follows the [SignPath Foundation conditions for Open Source
projects](https://signpath.org/terms.html).
