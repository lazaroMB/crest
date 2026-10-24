-- crest configuration.
--
-- Everything here is commented out, so crest runs with its defaults. Remove
-- the `--` in front of a line to use it, then reopen crest. Every option is
-- explained in docs/configuration.md, themes in docs/themes.md:
--   https://github.com/lazaroMB/crest/blob/main/docs/configuration.md
--
-- The installers created this file only because there was none: updating
-- crest never overwrites it.

local app = require("app")

-- How the window opens: "fullscreen" (the default; "maximized" on
-- Windows), "maximized" or "windowed". F11 toggles fullscreen at any time.
-- app.opt.window = "maximized"
-- app.opt.window = { mode = "windowed", width = 1440, height = 900 }

-- The color theme: "twocrest" (dark, the default), "twocrest-light", or
-- any file in the themes/ folder next to this one, by its name without
-- .lua. /theme previews them and saves your pick; a theme set here wins.
-- app.opt.theme = "tokyonight-night"

-- Tweak any color of the theme in use (see docs/themes.md for the tokens).
-- app.opt.colors = {
--   ui = { caret = "#ff9e64", list = { selected = { border = "#ff9e64" } } },
--   syntax = { comment = { italic = false } },
-- }

-- The key of <leader> in your keymaps.
-- app.opt.leader = "<C-g>"

-- The shell of /! commands.
-- app.opt.shell = "bash"

-- A keymap and a command of your own.
-- app.keymap.set("<leader>s", function() app.cmd("standup") end)
-- app.commands.register("standup", function()
--   app.send("Summarize what changed since yesterday from `git log`.", { role = "plan" })
-- end, { desc = "Draft my standup" })
