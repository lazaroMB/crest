# Installing crest in one step

Research of 2026-10-24 (web sources, each claim checked by three
independent verifiers) on how a solo-maintained, GPL GPUI app should
distribute itself on Linux, macOS and Windows, and what was built from it.

## Findings

- **Zed, the closest reference (also GPUI), keeps three official
  channels**: `curl -f https://zed.dev/install.sh | sh` on Linux (x86_64
  and aarch64, `ZED_VERSION` / `ZED_CHANNEL`), a Homebrew cask on macOS,
  and winget on Windows (x64 and Arm64). Flathub, Arch, Fedora, Nix and
  the rest are community packages, labeled unofficial.
  ([installation](https://zed.dev/docs/installation),
  [linux](https://zed.dev/docs/linux.html))
- **Many first-party Linux channels are expensive.** WezTerm ships Flathub,
  AppImage, its own APT repo, .deb/.rpm, Copr, a Linuxbrew tap, a Nix
  flake and a tarball from CI, plus distro packagers. Not worth it alone.
  ([wezterm linux](https://wezterm.org/install/linux.html))
- **Flatpak is a bad primary channel for a coding agent**: the sandbox
  hides the host's rg, git and toolchains unless the app goes through
  `flatpak-spawn --host` with `talk-name=org.freedesktop.Flatpak`, which
  defeats the sandbox. WezTerm warns its Flatpak breaks process
  inspection. ([flatpak-spawn](https://man7.org/linux/man-pages/man1/flatpak-spawn.1.html))
- **Homebrew 5.0.0 (2025-11-12) deprecated casks that fail Gatekeeper**
  (unsigned or not notarized) and `--no-quarantine`; such casks were
  disabled in homebrew/cask on 2026-09-01 (CopyQ's was). An unsigned app
  can only live in a personal tap whose cask strips the quarantine itself.
  ([Workbrew on 5.0.0](https://workbrew.com/blog/homebrew-5-0-0))
- **macOS signing and notarization run without a Mac** with rcodesign
  (apple-codesign): it signs Mach-O and bundles, notarizes and staples.
  It needs the Apple Developer Program (99 USD/year) for the Developer ID
  certificate and an App Store Connect API key.
  ([apple-codesign](https://gregoryszorc.com/docs/apple-codesign/main/))
- **SignPath Foundation signs OSS for free**: OSI license without a
  commercial dual license, no proprietary parts, active, already released,
  built from the public repository. The certificate is the Foundation's,
  so Windows shows "SignPath Foundation" as the publisher. The
  application also asks for proof of reputation (downloads, media, community
  use), which a new project does not have yet: deferred until it does.
  ([signpath.org](https://signpath.org/), [terms](https://signpath.org/terms))
- **Azure Artifact Signing** (formerly Trusted Signing) is the paid
  alternative with one's own identity: Basic 9.99 USD/month, 5,000
  signatures. Eligibility outside the US/Canada for individuals is
  unclear. ([pricing](https://azure.microsoft.com/pricing/details/artifact-signing/))
- **winget updates can be automated with Komac** once a first version is
  in winget-pkgs (sent by hand), with a fork and a classic token. Pin
  Komac: the `winget-releaser` action always pulls the latest Komac, which
  broke Databricks' releases. ([WinGet Updater](https://github.com/marketplace/actions/winget-updater))
- **Packaging tools**: cargo-packager (CrabNebula, from Tauri's bundler)
  makes .app/.dmg, .deb, AppImage, NSIS and WiX .msi from one config and
  has an updater crate; it neither signs nor notarizes, nor makes .rpm,
  Flatpak or MSIX. dist (cargo-dist) orchestrates tag releases with shell
  and PowerShell installers and the axoupdater self-updater (which only
  updates dist-installed copies). Velopack is a Squirrel/Sparkle-like
  installer and updater with a Rust crate, but its packer needs .NET.
  ([cargo-packager](https://github.com/crabnebula-dev/cargo-packager),
  [axoupdater](https://docs.rs/crate/axoupdater/latest),
  [Velopack](https://docs.velopack.io/getting-started/rust))

## What was built (phases 0 and 1)

| Platform | One step | Signing |
| --- | --- | --- |
| macOS | `brew install --cask lazaromb/tap/crest` | Developer ID + notarization (`scripts/sign-macos.sh`, rcodesign 0.29.0) |
| Windows | `winget install LazaroMB.Crest` or `irm … \| iex` | none yet (SignPath steps wired, skipped without secrets) |
| Linux | `curl … install.sh \| sh`, now also aarch64 | checksums only |

- crest.app's executable is the binary itself (a shell script as the main
  executable does not notarize well); it re-executes through the login
  shell when LaunchServices opens it (`crates/platform/src/login_shell.rs`).
- Windows gets an Inno Setup installer (`packaging/windows/crest.iss`,
  Inno Setup 6 ships with the windows-2025 runner): per user without
  admin rights, Start menu entry, optional desktop icon, Settings > Apps
  entry with icon and uninstaller, `crest` on the PATH, in-place upgrades.
  winget and `install.ps1` both run it; the zip stays as the portable
  build. The uninstaller Inno writes at install time is not signed.
- Unsigned on Windows, the paths without SmartScreen are winget and the
  PowerShell command: SmartScreen checks files carrying the browser's
  Mark of the Web, and what `Invoke-WebRequest` or winget downloads has
  none. Smart App Control (Windows 11, fresh installs) still blocks
  unsigned apps. A downloader `.exe` would not help: it would be an
  unsigned download itself.
- Manual, once: Apple Developer account and secrets, the `lazaroMB/homebrew-tap` repository, the first winget
  submission (`packaging/winget/README.md`), the `PACKAGES_TOKEN` secret.

## Later

- **Phase 2, native installers**: .dmg, .deb and AppImage (cargo-packager
  or by hand), signed by the same steps. Windows already has its setup.
- **Phase 3, updates**: package managers already update; for script
  installs, an in-app check against GitHub Releases (cargo-packager-updater
  or our own, since axoupdater needs dist receipts).
- **The official homebrew/cask** once the repository meets its notability
  criteria (the app is already notarized).
- **Community channels**: AUR, nixpkgs, Flathub (the latter with the
  `flatpak-spawn --host` caveat). Not maintained here.
- **Windows on Arm**: not built; GPUI support there is unverified.

## Open questions

- How long SmartScreen takes to trust a SignPath-signed binary compared to
  an EV certificate.
- Whether an individual outside the US/Canada can use Azure Artifact
  Signing, if we ever want our own publisher name.
- The oldest glibc and Vulkan/Wayland stack a GPUI AppImage can run on.
