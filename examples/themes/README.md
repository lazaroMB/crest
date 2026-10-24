# Themes

Color themes for crest, ported from popular Vim and Neovim colorschemes. The
colors come from each upstream repository (the file header names the files
and commit they were taken from); the mapping onto crest's tokens follows how
the upstream theme colors its Neovim highlight groups.

## Install

Copy the `.lua` files into your themes folder:

- Linux and macOS: `~/.config/crest/themes/`
- Windows: `%APPDATA%\crest\themes\`

The crest installers copy them there for you, and never overwrite a file you
have edited.

Then pick a theme with `/theme`, or set it in `init.lua` by its file name
without `.lua`:

```lua
app.opt.theme = "tokyonight-night"
```

## Themes

| Theme | Appearance | Upstream | License |
| --- | --- | --- | --- |
| `catppuccin-mocha` | dark | [catppuccin/nvim](https://github.com/catppuccin/nvim) | MIT |
| `catppuccin-latte` | light | [catppuccin/nvim](https://github.com/catppuccin/nvim) | MIT |
| `tokyonight-night` | dark | [folke/tokyonight.nvim](https://github.com/folke/tokyonight.nvim) | Apache-2.0 |
| `tokyonight-day` | light | [folke/tokyonight.nvim](https://github.com/folke/tokyonight.nvim) | Apache-2.0 |
| `kanagawa-wave` | dark | [rebelot/kanagawa.nvim](https://github.com/rebelot/kanagawa.nvim) | MIT |
| `rose-pine` | dark | [rose-pine/neovim](https://github.com/rose-pine/neovim) | MIT |
| `rose-pine-dawn` | light | [rose-pine/neovim](https://github.com/rose-pine/neovim) | MIT |
| `gruvbox-dark` | dark | [ellisonleao/gruvbox.nvim](https://github.com/ellisonleao/gruvbox.nvim) (palette from [morhetz/gruvbox](https://github.com/morhetz/gruvbox)) | MIT |
| `gruvbox-light` | light | [ellisonleao/gruvbox.nvim](https://github.com/ellisonleao/gruvbox.nvim) (palette from [morhetz/gruvbox](https://github.com/morhetz/gruvbox)) | MIT |
| `nightfox` | dark | [EdenEast/nightfox.nvim](https://github.com/EdenEast/nightfox.nvim) | MIT |
| `onedark` | dark | [navarasu/onedark.nvim](https://github.com/navarasu/onedark.nvim) (`dark` style) | MIT |
| `dracula` | dark | [dracula/vim](https://github.com/dracula/vim) | MIT |
| `nord` | dark | [nordtheme/vim](https://github.com/nordtheme/vim) | MIT |
| `everforest-dark` | dark | [sainnhe/everforest](https://github.com/sainnhe/everforest) (medium background) | MIT |

Each file keeps the copyright notice of its upstream project in its header.

## Write your own

[`_template.lua`](_template.lua) lists every color token and syntax name,
commented out, with what each one paints and what it takes when you leave it
unset. Copy it under a new name, set `name` to match, and fill in what you
need: `background`, `foreground` and `accent` alone make a complete theme.
[docs/themes.md](../../docs/themes.md) explains the format in full.
