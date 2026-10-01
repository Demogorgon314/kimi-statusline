<div align="center">

# kimi-statusline

**See what your Kimi Code session is really costing you — tokens, cache hits and plan quota — right in the footer.**

[![Release](https://img.shields.io/github/v/release/Demogorgon314/kimi-statusline?style=flat-square)](https://github.com/Demogorgon314/kimi-statusline/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/Demogorgon314/kimi-statusline/ci.yml?branch=main&style=flat-square&label=ci)](https://github.com/Demogorgon314/kimi-statusline/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
![Rust](https://img.shields.io/badge/rust-%E2%9C%93-orange?style=flat-square&logo=rust)

English | [中文](README.zh.md)

![kimi-statusline in Kimi Code](assets/hero.png)

</div>

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.sh | sh
```

Then run `/reload-tui` in Kimi Code. That's it.

<details>
<summary>Windows, Kimi plugin, or from source</summary>

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.ps1 | iex
```

**As a Kimi Code plugin** — inside Kimi Code:

```
/plugins install https://github.com/Demogorgon314/kimi-statusline
/reload
/new
/reload-tui
```

**From source**

```bash
cargo install --git https://github.com/Demogorgon314/kimi-statusline
kimi-statusline install
```

Requires kimi-code ≥ 0.30.0. The installer never overwrites someone else's status line command (use `install --force`), keeps your `tui.toml` comments, and backs it up to `tui.toml.bak`.

</details>

## Why

Kimi Code's footer tells you the model and the directory. It doesn't tell you:

- 📊 **How many tokens the whole session burned** — main agent *and* every sub-agent — and your **cache hit rate**, colored red → green
- ⏳ **How much of your 5-hour and 7-day quota is left**, and when it resets (`5h 42% ↻1h20m · 7d 63% ↻3d`)
- 🧩 **Which sub-agent model is the expensive one**
- 🌿 **Your git state at a glance** — diff stats, ahead/behind, and a clickable `[PR#42]`

kimi-statusline adds all of that while keeping everything the built-in footer shows: mode, goal, model + thinking effort, background tasks — even the `/dance` rainbow.

## Make it yours

![Built-in themes](assets/themes.png)

10 built-in themes, from a look that blends into Kimi Code to full powerline. Run `kimi-statusline` to open the configurator:

![TUI configurator](assets/configurator.png)

Toggle and reorder segments, pick colors and icons, switch themes — with a live preview of your own session. Works with the keyboard or the mouse: click to select, click again to toggle or edit, scroll to move. Inspired by [CCometixLine](https://github.com/Haleclipse/CCometixLine).

## Fast by design

Kimi Code runs the status line every second and kills it after 300 ms. kimi-statusline is a single Rust binary that renders in **~10–20 ms**:

- Session logs are read **incrementally** — a 100 MB session costs the same as a fresh one
- Network calls (quota, `gh pr view`) run **in the background**; the status line only reads caches
- Narrow terminal? It **compacts and drops** low-priority segments instead of getting cut off

![Adaptive width](assets/adaptive.png)

## Reference

<details>
<summary>Segments</summary>

| id | Shows |
| --- | --- |
| `mode` | Permission mode / plan / swarm / tower badges |
| `goal` | `[goal ● active · 4m · 7 turns]` |
| `model` | Model and thinking effort (follows `/effort`); rainbow after `/dance` |
| `tasks` | `[2 tasks running]` / `[1 agent running]` |
| `directory` | Working directory |
| `git` | Branch, diff stats, ahead/behind, open PR (needs `gh`) |
| `context` | Context window fill (off by default — the footer's second line shows it) |
| `usage` | Whole-session input ↑ / output ↓ / cache hit rate |
| `subagent` | Same, for the heaviest sub-agent model |
| `session` | Session age (off by default) |
| `quota` | 5h / 7d (optionally monthly) usage and reset time |

When space runs out, segments drop in this order: session → git → directory → subagent → tasks → goal → context → quota → mode.

</details>

<details>
<summary>Configuration file</summary>

`~/.kimi-code/kimi-statusline/config.toml` (custom themes in `themes/<name>.toml`), same shape as CCometixLine:

```toml
theme = "kimi"

[style]
mode = "plain"          # plain | nerd_font | powerline
separator = "  "        # "" for powerline arrows
lang = "en"             # or "zh"
palette = ""            # "" follows tui.toml; or dark / light / a Kimi theme name

[[segments]]
id = "quota"
enabled = true
colors = { text = "text_dim" }   # c16 / c256 / RGB / "#rrggbb" / Kimi palette name
options = { show_5h = true, show_7d = true, show_month = false, bar = false, refresh_secs = 120 }
```

Kimi palette names (`primary`, `accent`, `text_dim`, `success`, `warning`, `error`, …) follow your TUI theme, dark or light.

</details>

<details>
<summary>How quota works</summary>

It calls the same endpoint as Kimi Code's `/usage`, with the login Kimi Code already stores in `~/.kimi-code/credentials/`. Requests run in the background at most every `refresh_secs`.

kimi-statusline never refreshes your login itself — racing Kimi Code's token rotation could log you out. If Kimi Code sits idle long enough for its token to expire, the quota holds its last value and updates after your next message. `kimi-statusline quota` fetches it on demand.

</details>

<details>
<summary>Commands, debugging, uninstall</summary>

```bash
kimi-statusline                 # menu: configure, install, test quota…
kimi-statusline config          # configurator
kimi-statusline themes          # list themes
kimi-statusline -t nord preview # render a theme in your terminal
kimi-statusline quota           # fetch quota now
kimi-statusline uninstall       # remove from tui.toml, then /reload-tui
```

- Debug log: `touch ~/.kimi-code/kimi-statusline-debug`, then read `~/.kimi-code/kimi-statusline-debug.log`
- Plugin users: `/plugins remove kimi-statusline` — the status line cleans up `tui.toml` by itself

</details>

## Contributing

```bash
cargo test
python3 scripts/screenshots.py   # regenerate assets/ (Chrome, ImageMagick, a Nerd Font, pyte)
```

To release, bump the version in `Cargo.toml` and `kimi.plugin.json` and push a `vX.Y.Z` tag.

Credits: [CCometixLine](https://github.com/Haleclipse/CCometixLine) for the configurator and theme design, [kimi-usage](https://github.com/YD-233/kimi-usage) for session parsing.

## License

MIT
