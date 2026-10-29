# crest on winget

`winget install LazaroMB.Crest` runs the release's Inno Setup installer
(`crest-<version>-x86_64-pc-windows-msvc-setup.exe`, from
`packaging/windows/crest.iss`) silently, per user: crest gets its Start
menu entry, its place in Settings > Apps and the `crest` command on the
`PATH`, and `winget upgrade` installs the next setup over it.

The `winget` job of `.github/workflows/release.yml` sends each new release
with [Komac](https://github.com/russellbanks/Komac), pinned by version and
SHA-256. It updates a package that already exists, so the **first version
is sent by hand**, once:

1. Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)
   and create a classic personal access token with the `public_repo` scope.
   Save it as the `PACKAGES_TOKEN` secret of lazaroMB/crest (the `homebrew`
   job pushes to the tap with it too).
2. Install Komac (`winget install RussellBanks.Komac`, or the release
   binary) and run:

   ```sh
   komac new LazaroMB.Crest --version 0.18.0 \
     --urls https://github.com/lazaroMB/crest/releases/download/v0.18.0/crest-0.18.0-x86_64-pc-windows-msvc-setup.exe \
     --token "$PACKAGES_TOKEN"
   ```

   Use the latest non-prerelease tag. Answer its questions with:

   | Field | Value |
   | --- | --- |
   | PackageIdentifier | `LazaroMB.Crest` |
   | Publisher | `TwoCrest Systems` |
   | PackageName | `crest` |
   | License | `GPL-3.0-or-later` |
   | ShortDescription | `Desktop coding agent for Claude and OpenCode models` |
   | PackageUrl | `https://github.com/lazaroMB/crest` |
   | InstallerType | `inno` (Komac detects it) |
   | Scope | `user` |
   | UpgradeBehavior | `install` |
   | Dependencies | `BurntSushi.ripgrep.MSVC`, `junegunn.fzf`, `Git.Git` |

3. Komac opens the pull request. On a first contribution to a Microsoft
   repo a bot asks you to accept its CLA with a comment on the pull request.
   Microsoft's bots validate it and a moderator merges it (a few days the
   first time).

From then on, a `v*` tag that is not a prerelease updates the package:
`komac update LazaroMB.Crest --version <v> --urls <setup.exe> --submit`. Each
update is still a pull request reviewed by Microsoft. Without the
`PACKAGES_TOKEN` secret, or while `LazaroMB.Crest` is not yet in
winget-pkgs, the job is skipped with a notice.
